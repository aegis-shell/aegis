// hidpi_probe — replicate Chrome's fractional-scale commit pattern:
//   viewport_dst = logical size, buffer at 2x, buffer_scale=1,
//   wl_surface.damage in surface-local logical coords, alternating buffers.
// Fills left half green, right half red; second frame: right half blue.
// The driving test inspects the compositor's snapshot pixels directly.
#define _GNU_SOURCE
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/mman.h>
#include <wayland-client.h>
#include "xdg-shell-client.h"
#include "viewporter-client.h"
#include "fractional-scale-v1-client.h"

static struct wl_display *g_display;
static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct xdg_wm_base *wm_base;
static struct wp_viewporter *viewporter;
static struct wp_fractional_scale_manager_v1 *fs_manager;

#define LOGICAL_W 300
#define LOGICAL_H 40
#define SCALE 2
#define BUF_W (LOGICAL_W * SCALE)
#define BUF_H (LOGICAL_H * SCALE)

static void log_line(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void log_line(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    fputc('\n', stdout);
    fflush(stdout);
}

static struct wl_buffer *bufs[2];
static int buf_fd[2];

static void make_buffers() {
    int size = BUF_W * BUF_H * 4;
    for (int i = 0; i < 2; i++) {
        int fd = memfd_create("hidpi", MFD_CLOEXEC);
        ftruncate(fd, size);
        buf_fd[i] = fd;
        struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, size);
        bufs[i] = wl_shm_pool_create_buffer(pool, 0, BUF_W, BUF_H, BUF_W * 4,
                                            WL_SHM_FORMAT_ARGB8888);
        wl_shm_pool_destroy(pool);
    }
}

static void paint(int i, uint32_t left, uint32_t right) {
    int size = BUF_W * BUF_H * 4;
    uint32_t *data = mmap(NULL, size, PROT_WRITE, MAP_SHARED, buf_fd[i], 0);
    for (int y = 0; y < BUF_H; y++)
        for (int x = 0; x < BUF_W; x++)
            data[y * BUF_W + x] = x < BUF_W / 2 ? left : right;
    munmap(data, size);
}

static struct wl_surface *tl_surface, *pu_surface;
static struct xdg_surface *tl_xdg, *pu_xdg;
static struct xdg_toplevel *tl_toplevel;
static struct wp_viewport *pu_viewport;
static int tl_configured = 0, popup_ready = 0, frames_sent = 0;

static void pu_xdg_configure(void *data, struct xdg_surface *s, uint32_t serial) {
    (void)data;
    xdg_surface_ack_configure(s, serial);
    popup_ready = 1;
    log_line("popup-xdg-configure");
}
static const struct xdg_surface_listener pu_xdg_listener = { .configure = pu_xdg_configure };

static void popup_configure(void *d, struct xdg_popup *p, int32_t x, int32_t y, int32_t w, int32_t h) {
    (void)d; (void)p;
    log_line("popup-configure x=%d y=%d w=%d h=%d", x, y, w, h);
}
static void popup_done(void *d, struct xdg_popup *p) { (void)d; (void)p; }
static const struct xdg_popup_listener popup_listener = {
    .configure = popup_configure, .popup_done = popup_done,
};

static void tl_xdg_configure(void *data, struct xdg_surface *s, uint32_t serial) {
    (void)data;
    xdg_surface_ack_configure(s, serial);
    tl_configured = 1;
    log_line("toplevel-configured");
}
static const struct xdg_surface_listener tl_xdg_listener = { .configure = tl_xdg_configure };
static void tl_configure(void *d, struct xdg_toplevel *t, int32_t w, int32_t h, struct wl_array *st) {
    (void)d;(void)t;(void)st;(void)w;(void)h;
}
static void tl_close(void *d, struct xdg_toplevel *t) { (void)d;(void)t; }
static const struct xdg_toplevel_listener tl_listener = { .configure = tl_configure, .close = tl_close };

static void fs_preferred(void *data, struct wp_fractional_scale_v1 *fs, uint32_t scale) {
    (void)data; (void)fs;
    log_line("preferred-scale %u", scale);
}
static const struct wp_fractional_scale_v1_listener fs_listener = { .preferred_scale = fs_preferred };

static void registry_add(void *data, struct wl_registry *r, uint32_t id, const char *iface, uint32_t version) {
    (void)data;
    if (strcmp(iface, wl_compositor_interface.name) == 0)
        compositor = wl_registry_bind(r, id, &wl_compositor_interface, 4);
    else if (strcmp(iface, wl_shm_interface.name) == 0)
        shm = wl_registry_bind(r, id, &wl_shm_interface, 1);
    else if (strcmp(iface, xdg_wm_base_interface.name) == 0)
        wm_base = wl_registry_bind(r, id, &xdg_wm_base_interface, 1);
    else if (strcmp(iface, "wp_viewporter") == 0)
        viewporter = wl_registry_bind(r, id, &wp_viewporter_interface, 1);
    else if (strcmp(iface, "wp_fractional_scale_manager_v1") == 0)
        fs_manager = wl_registry_bind(r, id, &wp_fractional_scale_manager_v1_interface, 1);
}
static void registry_remove(void *d, struct wl_registry *r, uint32_t id) { (void)d;(void)r;(void)id; }
static const struct wl_registry_listener registry_listener = { .global = registry_add, .global_remove = registry_remove };

int main(void) {
    g_display = wl_display_connect(NULL);
    if (!g_display) return 1;
    struct wl_registry *reg = wl_display_get_registry(g_display);
    wl_registry_add_listener(reg, &registry_listener, NULL);
    wl_display_roundtrip(g_display);
    if (!compositor || !shm || !wm_base || !viewporter) {
        fprintf(stderr, "missing globals\n");
        return 1;
    }
    make_buffers();

    tl_surface = wl_compositor_create_surface(compositor);
    tl_xdg = xdg_wm_base_get_xdg_surface(wm_base, tl_surface);
    xdg_surface_add_listener(tl_xdg, &tl_xdg_listener, NULL);
    tl_toplevel = xdg_surface_get_toplevel(tl_xdg);
    xdg_toplevel_add_listener(tl_toplevel, &tl_listener, NULL);
    xdg_toplevel_set_title(tl_toplevel, "hidpi-probe");
    wl_surface_commit(tl_surface);
    wl_display_roundtrip(g_display);

    // Toplevel buffer (solid dark).
    {
        int w = 400, h = 300;
        int fd = memfd_create("t", MFD_CLOEXEC);
        ftruncate(fd, w * h * 4);
        uint32_t *data = mmap(NULL, w * h * 4, PROT_WRITE, MAP_SHARED, fd, 0);
        for (int i = 0; i < w * h; i++) data[i] = 0xff202020;
        munmap(data, w * h * 4);
        struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, w * h * 4);
        struct wl_buffer *b = wl_shm_pool_create_buffer(pool, 0, w, h, w * 4, WL_SHM_FORMAT_ARGB8888);
        wl_surface_attach(tl_surface, b, 0, 0);
        wl_surface_damage(tl_surface, 0, 0, w, h);
        wl_surface_commit(tl_surface);
        wl_shm_pool_destroy(pool);
        close(fd);
    }
    wl_display_roundtrip(g_display);

    // Popup with Chrome's exact pattern.
    pu_surface = wl_compositor_create_surface(compositor);
    if (fs_manager) {
        struct wp_fractional_scale_v1 *fs =
            wp_fractional_scale_manager_v1_get_fractional_scale(fs_manager, pu_surface);
        wp_fractional_scale_v1_add_listener(fs, &fs_listener, NULL);
    }
    pu_viewport = wp_viewporter_get_viewport(viewporter, pu_surface);
    struct xdg_positioner *pos = xdg_wm_base_create_positioner(wm_base);
    xdg_positioner_set_size(pos, LOGICAL_W, LOGICAL_H);
    xdg_positioner_set_anchor_rect(pos, 50, 50, 10, 10);
    xdg_positioner_set_anchor(pos, XDG_POSITIONER_ANCHOR_BOTTOM);
    xdg_positioner_set_gravity(pos, XDG_POSITIONER_GRAVITY_BOTTOM);
    pu_xdg = xdg_wm_base_get_xdg_surface(wm_base, pu_surface);
    xdg_surface_add_listener(pu_xdg, &pu_xdg_listener, NULL);
    struct xdg_popup *popup = xdg_surface_get_popup(pu_xdg, tl_xdg, pos);
    xdg_popup_add_listener(popup, &popup_listener, NULL);
    xdg_positioner_destroy(pos);
    wl_surface_commit(pu_surface);
    wl_display_roundtrip(g_display);
    if (!popup_ready) { fprintf(stderr, "no popup configure\n"); return 1; }

    wp_viewport_set_destination(pu_viewport, LOGICAL_W, LOGICAL_H);
    for (frames_sent = 0; frames_sent < 4; frames_sent++) {
        int i = frames_sent % 2;
        paint(i, 0xff00cc00, frames_sent == 3 ? 0xff0033cc : 0xffcc0000);
        wl_surface_attach(pu_surface, bufs[i], 0, 0);
        wl_surface_damage(pu_surface, 0, 0, LOGICAL_W, LOGICAL_H);
        wl_surface_commit(pu_surface);
        wl_display_flush(g_display);
        // Let the driver pump between frames.
        wl_display_dispatch_pending(g_display);
        usleep(30000);
    }
    log_line("frames-done");
    // Stay alive until killed.
    while (wl_display_dispatch(g_display) != -1) {}
    return 0;
}
