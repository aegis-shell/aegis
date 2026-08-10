// popup_probe — minimal xdg-shell client for probing compositor popup behavior.
//
// Modes:
//   menu            Open a grabbed popup when the toplevel receives its first
//                   button release; log every pointer event, popup configure
//                   and popup_done.
//   tooltip X Y     Immediately open a non-grabbed popup whose positioner
//                   anchor rect sits at toplevel-local (X, Y); log the
//                   xdg_popup.configure the compositor sends back.
//
// All log lines go to stdout, newline-delimited and flushed, so a driving
// harness can assert on them.
//
// Build:
//   wayland-scanner client-header $XDG_XML xdg-shell-client.h
//   wayland-scanner private-code  $XDG_XML xdg-shell-protocol.c
//   gcc -o popup_probe popup_probe.c xdg-shell-protocol.c \
//       $(pkg-config --cflags --libs wayland-client)

#define _GNU_SOURCE
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <wayland-client.h>
#include "xdg-shell-client.h"

static struct wl_display *g_display;
static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct wl_seat *seat;
static struct wl_pointer *pointer;
static struct xdg_wm_base *wm_base;

static struct wl_surface *tl_surface;
static struct xdg_surface *tl_xdg;
static struct xdg_toplevel *tl_toplevel;
static int tl_configured = 0;
static int tl_w = 640, tl_h = 480;

static struct wl_surface *pu_surface;
static struct xdg_surface *pu_xdg;
static struct xdg_popup *pu_popup;
static int pu_w = 220, pu_h = 160;

static const char *mode;
static int tooltip_ax = 100, tooltip_ay = 100;
static int popup_created = 0;

static void log_line(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void log_line(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    fputc('\n', stdout);
    fflush(stdout);
}

// ---------- shm buffer helpers ----------

static struct wl_buffer *make_buffer(int w, int h, uint32_t argb) {
    int stride = w * 4;
    int size = stride * h;
    int fd = memfd_create("probe", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, size) < 0) return NULL;
    uint32_t *data = mmap(NULL, size, PROT_WRITE, MAP_SHARED, fd, 0);
    if (data == MAP_FAILED) return NULL;
    for (int i = 0; i < w * h; i++) data[i] = argb;
    munmap(data, size);
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, size);
    struct wl_buffer *buf =
        wl_shm_pool_create_buffer(pool, 0, w, h, stride, WL_SHM_FORMAT_ARGB8888);
    wl_shm_pool_destroy(pool);
    close(fd);
    return buf;
}

// ---------- popup ----------

static void popup_configure(void *data, struct xdg_popup *p, int32_t x, int32_t y,
                            int32_t w, int32_t h) {
    (void)data; (void)p;
    log_line("popup-configure x=%d y=%d w=%d h=%d", x, y, w, h);
}

static void popup_done(void *data, struct xdg_popup *p) {
    (void)data; (void)p;
    log_line("popup-done");
}

static const struct xdg_popup_listener popup_listener = {
    .configure = popup_configure,
    .popup_done = popup_done,
};

static void pu_xdg_configure(void *data, struct xdg_surface *s, uint32_t serial) {
    (void)data;
    xdg_surface_ack_configure(s, serial);
    struct wl_buffer *buf = make_buffer(pu_w, pu_h, 0xffcc3333);
    wl_surface_attach(pu_surface, buf, 0, 0);
    wl_surface_damage(pu_surface, 0, 0, pu_w, pu_h);
    wl_surface_commit(pu_surface);
    wl_display_flush(g_display);
    log_line("popup-mapped w=%d h=%d", pu_w, pu_h);
}

static const struct xdg_surface_listener pu_xdg_listener = {
    .configure = pu_xdg_configure,
};

static void create_popup(uint32_t grab_serial) {
    struct xdg_positioner *pos = xdg_wm_base_create_positioner(wm_base);
    if (strcmp(mode, "tooltip") == 0) {
        pu_w = 300; pu_h = 40;
        xdg_positioner_set_size(pos, pu_w, pu_h);
        xdg_positioner_set_anchor_rect(pos, tooltip_ax, tooltip_ay, 10, 10);
        xdg_positioner_set_anchor(pos, XDG_POSITIONER_ANCHOR_BOTTOM);
        xdg_positioner_set_gravity(pos, XDG_POSITIONER_GRAVITY_BOTTOM);
        xdg_positioner_set_constraint_adjustment(
            pos, XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_SLIDE_X |
                 XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_SLIDE_Y |
                 XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_FLIP_X |
                 XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_FLIP_Y);
    } else {
        // Menu grabbing like Qt/GTK menus do, anchored so its top-left
        // corner lands exactly on the triggering click point: the cursor is
        // over the popup from the moment it maps, with no motion in between.
        xdg_positioner_set_size(pos, pu_w, pu_h);
        xdg_positioner_set_anchor_rect(pos, 90, 90, 10, 10);
        xdg_positioner_set_anchor(pos, XDG_POSITIONER_ANCHOR_TOP_LEFT);
        xdg_positioner_set_gravity(pos, XDG_POSITIONER_GRAVITY_BOTTOM_RIGHT);
        xdg_positioner_set_constraint_adjustment(
            pos, XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_SLIDE_X |
                 XDG_POSITIONER_CONSTRAINT_ADJUSTMENT_SLIDE_Y);
    }
    pu_surface = wl_compositor_create_surface(compositor);
    pu_xdg = xdg_wm_base_get_xdg_surface(wm_base, pu_surface);
    xdg_surface_add_listener(pu_xdg, &pu_xdg_listener, NULL);
    pu_popup = xdg_surface_get_popup(pu_xdg, tl_xdg, pos);
    xdg_popup_add_listener(pu_popup, &popup_listener, NULL);
    xdg_positioner_destroy(pos);
    if (grab_serial) {
        xdg_popup_grab(pu_popup, seat, grab_serial);
        log_line("popup-grab-requested serial=%u", grab_serial);
    }
    wl_surface_commit(pu_surface);
    wl_display_flush(g_display);
}

// ---------- toplevel ----------

static void tl_xdg_configure(void *data, struct xdg_surface *s, uint32_t serial) {
    (void)data;
    xdg_surface_ack_configure(s, serial);
    if (!tl_configured) {
        tl_configured = 1;
        struct wl_buffer *buf = make_buffer(tl_w, tl_h, 0xff202020);
        wl_surface_attach(tl_surface, buf, 0, 0);
        wl_surface_damage(tl_surface, 0, 0, tl_w, tl_h);
        wl_surface_commit(tl_surface);
        wl_display_flush(g_display);
        log_line("toplevel-mapped w=%d h=%d", tl_w, tl_h);
    }
}

static const struct xdg_surface_listener tl_xdg_listener = {
    .configure = tl_xdg_configure,
};

static void tl_configure(void *data, struct xdg_toplevel *t, int32_t w, int32_t h,
                         struct wl_array *states) {
    (void)data; (void)t; (void)states;
    if (w > 0 && h > 0) { tl_w = w; tl_h = h; }
    log_line("toplevel-configure w=%d h=%d", w, h);
}

static void tl_close(void *data, struct xdg_toplevel *t) { (void)data; (void)t; }

static const struct xdg_toplevel_listener tl_listener = {
    .configure = tl_configure,
    .close = tl_close,
};

// ---------- pointer ----------

static const char *surface_name(struct wl_surface *s) {
    if (s == tl_surface) return "toplevel";
    if (s == pu_surface) return "popup";
    return "other";
}

static void ptr_enter(void *data, struct wl_pointer *p, uint32_t serial,
                      struct wl_surface *s, wl_fixed_t x, wl_fixed_t y) {
    (void)data; (void)p;
    log_line("enter surface=%s serial=%u x=%.2f y=%.2f", surface_name(s), serial,
             wl_fixed_to_double(x), wl_fixed_to_double(y));
}

static void ptr_leave(void *data, struct wl_pointer *p, uint32_t serial,
                      struct wl_surface *s) {
    (void)data; (void)p;
    log_line("leave surface=%s serial=%u", surface_name(s), serial);
}

static void ptr_motion(void *data, struct wl_pointer *p, uint32_t time,
                       wl_fixed_t x, wl_fixed_t y) {
    (void)data; (void)p; (void)time;
    static int motion_count = 0;
    // Throttle: log 1 in 20 motions so the harness can see delivery works.
    if (motion_count++ % 20 == 0)
        log_line("motion x=%.2f y=%.2f", wl_fixed_to_double(x), wl_fixed_to_double(y));
}

static void ptr_button(void *data, struct wl_pointer *p, uint32_t serial,
                       uint32_t time, uint32_t button, uint32_t state) {
    (void)data; (void)p; (void)time;
    // The surface is implicit: it is whichever surface currently has pointer
    // focus, i.e. the last surface logged by ptr_enter.
    log_line("button serial=%u button=0x%x state=%u", serial, button, state);
    if (state == WL_POINTER_BUTTON_STATE_RELEASED && !popup_created &&
        strcmp(mode, "menu") == 0) {
        popup_created = 1;
        create_popup(serial);
        wl_display_flush(g_display);
    }
}

static void ptr_axis(void *data, struct wl_pointer *p, uint32_t time, uint32_t axis,
                     wl_fixed_t value) {
    (void)data; (void)p; (void)time; (void)axis; (void)value;
}

static const struct wl_pointer_listener ptr_listener = {
    .enter = ptr_enter,
    .leave = ptr_leave,
    .motion = ptr_motion,
    .button = ptr_button,
    .axis = ptr_axis,
};

// ---------- registry ----------

static void seat_caps(void *data, struct wl_seat *s, uint32_t caps) {
    (void)data;
    if ((caps & WL_SEAT_CAPABILITY_POINTER) && !pointer) {
        pointer = wl_seat_get_pointer(s);
        wl_pointer_add_listener(pointer, &ptr_listener, NULL);
        log_line("seat-pointer-bound");
    }
}

static const struct wl_seat_listener seat_listener = { .capabilities = seat_caps };

static void wm_ping(void *data, struct xdg_wm_base *w, uint32_t serial) {
    (void)data;
    xdg_wm_base_pong(w, serial);
}

static const struct xdg_wm_base_listener wm_listener = { .ping = wm_ping };

static void registry_add(void *data, struct wl_registry *r, uint32_t id,
                         const char *iface, uint32_t version) {
    (void)data;
    if (strcmp(iface, wl_compositor_interface.name) == 0)
        compositor = wl_registry_bind(r, id, &wl_compositor_interface, 4);
    else if (strcmp(iface, wl_shm_interface.name) == 0)
        shm = wl_registry_bind(r, id, &wl_shm_interface, 1);
    else if (strcmp(iface, wl_seat_interface.name) == 0) {
        seat = wl_registry_bind(r, id, &wl_seat_interface, 1);
        wl_seat_add_listener(seat, &seat_listener, NULL);
    } else if (strcmp(iface, xdg_wm_base_interface.name) == 0) {
        wm_base = wl_registry_bind(r, id, &xdg_wm_base_interface, 1);
        xdg_wm_base_add_listener(wm_base, &wm_listener, NULL);
    }
}

static void registry_remove(void *data, struct wl_registry *r, uint32_t id) {
    (void)data; (void)r; (void)id;
}

static const struct wl_registry_listener registry_listener = {
    .global = registry_add,
    .global_remove = registry_remove,
};

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: popup_probe menu|tooltip [X Y]\n");
        return 2;
    }
    mode = argv[1];
    if (strcmp(mode, "tooltip") == 0 && argc >= 4) {
        tooltip_ax = atoi(argv[2]);
        tooltip_ay = atoi(argv[3]);
    }
    g_display = wl_display_connect(NULL);
    if (!g_display) { fprintf(stderr, "cannot connect\n"); return 1; }
    struct wl_registry *reg = wl_display_get_registry(g_display);
    wl_registry_add_listener(reg, &registry_listener, NULL);
    wl_display_roundtrip(g_display);
    if (!compositor || !shm || !seat || !wm_base) {
        fprintf(stderr, "missing globals\n");
        return 1;
    }
    log_line("globals-ready");

    tl_surface = wl_compositor_create_surface(compositor);
    tl_xdg = xdg_wm_base_get_xdg_surface(wm_base, tl_surface);
    xdg_surface_add_listener(tl_xdg, &tl_xdg_listener, NULL);
    tl_toplevel = xdg_surface_get_toplevel(tl_xdg);
    xdg_toplevel_add_listener(tl_toplevel, &tl_listener, NULL);
    xdg_toplevel_set_title(tl_toplevel, "popup-probe");
    wl_surface_commit(tl_surface);
    wl_display_flush(g_display);

    while (wl_display_dispatch(g_display) != -1) {
        if (strcmp(mode, "tooltip") == 0 && tl_configured && !popup_created) {
            popup_created = 1;
            create_popup(0);
            wl_display_flush(g_display);
        }
    }
    return 0;
}
