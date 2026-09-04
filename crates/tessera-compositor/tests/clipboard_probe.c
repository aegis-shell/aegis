// clipboard_probe.c — a GUI-style wl_data_device clipboard client.
//
// Maps a toplevel, takes keyboard focus, then — depending on the mode
// argument — either sets or reads the selection through wl_data_device,
// exactly what a GTK/Qt application does on Ctrl+C / Ctrl+V.
//
//   copy  (default) create a wl_data_source, offer text/plain, call
//         set_selection with the keyboard enter serial. A clipboard manager
//         (wl-paste over ext-data-control) then reads it back: this
//         exercises the cross-family path where an ext_data_control_offer's
//         receive must marshal `send` for a *wl_data_source* (whose opcode
//         differs).
//   paste wait for the data device's selection event (something else —
//         wl-copy over ext-data-control — set the clipboard), then call
//         wl_data_offer.receive(text/plain, pipe) and log what arrives.
//         This exercises the reverse cross-family path: the wl_data_offer's
//         receive must marshal `send` for an *ext_data_control_source*.
//
// Log lines consumed by the Rust test:
//   probe-ready         registry parsed, globals bound
//   toplevel-mapped     buffer committed
//   keyboard-enter      focus serial observed
//   selection-set       set_selection sent (copy mode)
//   selection-received  data device saw a selection (paste mode)
//   pasted=<data>       receive() payload read back (paste mode)
//   source-send mime=.. wl_data_source served a send (copy mode)
//   source-cancelled    wl_data_source lost the selection (copy mode)

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <unistd.h>
#include <fcntl.h>
#include <poll.h>
#include <sys/mman.h>
#include <wayland-client.h>
#include "xdg-shell-client.h"

static struct wl_display *g_display;
static struct wl_compositor *g_compositor;
static struct wl_shm *g_shm;
static struct wl_seat *g_seat;
static struct wl_keyboard *g_keyboard;
static struct wl_data_device_manager *g_ddm;
static struct wl_data_device *g_data_device;
static struct xdg_wm_base *g_wm_base;

static struct wl_surface *g_surface;
static struct xdg_surface *g_xdg_surface;
static struct xdg_toplevel *g_toplevel;

static uint32_t g_enter_serial;
static int g_focused;
static int g_mapped;
static int g_selection_set;
static const char *g_mode = "copy";

static void log_line(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vfprintf(stderr, fmt, ap);
    va_end(ap);
    fputc('\n', stderr);
    fflush(stderr);
}

static void shm_buffer_for(struct wl_surface *s, int w, int h) {
    int stride = w * 4;
    int size = stride * h;
    char template[] = "/tmp/tessera-clip-probe-XXXXXX";
    int fd = mkstemp(template);
    unlink(template);
    ftruncate(fd, size);
    void *data = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    // Solid, fully opaque magenta.
    for (int i = 0; i < w * h; i++) {
        ((uint32_t *)data)[i] = 0xff00cc66;
    }
    struct wl_shm_pool *pool = wl_shm_create_pool(g_shm, fd, size);
    struct wl_buffer *buf =
        wl_shm_pool_create_buffer(pool, 0, w, h, stride, WL_SHM_FORMAT_XRGB8888);
    wl_surface_attach(s, buf, 0, 0);
    wl_surface_damage(s, 0, 0, w, h);
    wl_surface_commit(s);
    wl_shm_pool_destroy(pool);
    close(fd);
}

static void xdg_configure(void *data, struct xdg_surface *s, uint32_t serial) {
    (void)data;
    xdg_surface_ack_configure(s, serial);
    if (!g_mapped) {
        shm_buffer_for(g_surface, 400, 300);
        g_mapped = 1;
        log_line("toplevel-mapped");
    }
}

static const struct xdg_surface_listener xdg_listener = {.configure = xdg_configure};

static void kb_keymap(void *d, struct wl_keyboard *k, uint32_t f, int32_t fd, uint32_t size) {
    (void)d; (void)k; (void)f;
    char buf[128];
    while (size > 0) {
        ssize_t n = read(fd, buf, size > sizeof(buf) ? sizeof(buf) : size);
        if (n <= 0) break;
        size -= n;
    }
    close(fd);
}

static void kb_enter(void *d, struct wl_keyboard *k, uint32_t serial, struct wl_surface *s,
                    struct wl_array *keys) {
    (void)d; (void)k; (void)keys; (void)s;
    g_enter_serial = serial;
    g_focused = 1;
    log_line("keyboard-enter serial=%u", serial);
}

static void kb_leave(void *d, struct wl_keyboard *k, uint32_t serial, struct wl_surface *s) {
    (void)d; (void)k; (void)serial; (void)s;
    g_focused = 0;
}

static void kb_key(void *d, struct wl_keyboard *k, uint32_t serial, uint32_t time, uint32_t key,
                   uint32_t state) {
    (void)d; (void)k; (void)serial; (void)time; (void)key; (void)state;
}

static void kb_mods(void *d, struct wl_keyboard *k, uint32_t s1, uint32_t l1, uint32_t s2,
                    uint32_t l2, uint32_t g) {
    (void)d; (void)k; (void)s1; (void)l1; (void)s2; (void)l2; (void)g;
}

static void kb_repeat(void *d, struct wl_keyboard *k, int32_t r, int32_t dly) {
    (void)d; (void)k; (void)r; (void)dly;
}

static const struct wl_keyboard_listener kb_listener = {
    .keymap = kb_keymap,
    .enter = kb_enter,
    .leave = kb_leave,
    .key = kb_key,
    .modifiers = kb_mods,
    .repeat_info = kb_repeat,
};

static void seat_caps(void *d, struct wl_seat *s, uint32_t caps) {
    (void)d; (void)s;
    if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) && !g_keyboard) {
        g_keyboard = wl_seat_get_keyboard(g_seat);
        wl_keyboard_add_listener(g_keyboard, &kb_listener, NULL);
    }
}

static void seat_name(void *d, struct wl_seat *s, const char *name) {
    (void)d; (void)s; (void)name;
}

static const struct wl_seat_listener seat_listener = {.capabilities = seat_caps, .name = seat_name};

static void registry_global(void *d, struct wl_registry *r, uint32_t name, const char *iface,
                            uint32_t version) {
    (void)d;
    (void)version;
    if (!strcmp(iface, "wl_compositor")) {
        g_compositor = wl_registry_bind(r, name, &wl_compositor_interface, 1);
    } else if (!strcmp(iface, "wl_shm")) {
        g_shm = wl_registry_bind(r, name, &wl_shm_interface, 1);
    } else if (!strcmp(iface, "wl_seat")) {
        g_seat = wl_registry_bind(r, name, &wl_seat_interface, 2);
        wl_seat_add_listener(g_seat, &seat_listener, NULL);
    } else if (!strcmp(iface, "wl_data_device_manager")) {
        g_ddm = wl_registry_bind(r, name, &wl_data_device_manager_interface, 3);
    } else if (!strcmp(iface, "xdg_wm_base")) {
        g_wm_base = wl_registry_bind(r, name, &xdg_wm_base_interface, 1);
    }
}

static void registry_remove(void *d, struct wl_registry *r, uint32_t name) {
    (void)d; (void)r; (void)name;
}

static const struct wl_registry_listener registry_listener = {
    .global = registry_global,
    .global_remove = registry_remove,
};

static void source_send(void *data, struct wl_data_source *src, const char *mime, int32_t fd) {
    (void)data; (void)src;
    const char payload[] = "wl_data_device-side payload";
    if (mime && !strcmp(mime, "text/plain")) {
        if (write(fd, payload, sizeof(payload) - 1) < 0) {
            /* best effort */
        }
    }
    close(fd);
    log_line("source-send mime=%s", mime ? mime : "(null)");
}

static void source_cancelled(void *data, struct wl_data_source *src) {
    (void)data; (void)src;
    log_line("source-cancelled");
}

static const struct wl_data_source_listener source_listener = {
    .send = source_send,
    .cancelled = source_cancelled,
    .target = NULL,
    .dnd_drop_performed = NULL,
    .dnd_finished = NULL,
    .action = NULL,
};

// ----- paste mode: read a selection set by someone else through the
// wl_data_device family (a focused app's Ctrl+V path).

static struct wl_data_offer *g_incoming_offer;
static char **g_incoming_mimes;
static int g_incoming_mime_count;

static void offer_offer(void *data, struct wl_data_offer *o, const char *mime) {
    (void)data;
    char **grown = realloc(g_incoming_mimes, (g_incoming_mime_count + 1) * sizeof(char *));
    if (!grown) {
        return;
    }
    g_incoming_mimes = grown;
    g_incoming_mimes[g_incoming_mime_count++] = strdup(mime);
    (void)o;
}

static const struct wl_data_offer_listener offer_listener = {
    .offer = offer_offer,
    .source_actions = NULL,
    .action = NULL,
};

static void device_data_offer(void *data, struct wl_data_device *d, struct wl_data_offer *o) {
    (void)data; (void)d;
    g_incoming_offer = o;
    wl_data_offer_add_listener(o, &offer_listener, NULL);
}

static void device_selection(void *data, struct wl_data_device *d, struct wl_data_offer *o) {
    (void)data; (void)d;
    log_line("selection-received offer=%p", (void *)o);
    if (!o) {
        return;
    }
    // Ask for text/plain into a pipe, then log the payload.
    int fds[2];
    if (pipe(fds) != 0) {
        return;
    }
    wl_data_offer_receive(o, "text/plain", fds[1]);
    wl_display_flush(g_display);
    close(fds[1]);
    struct pollfd pfd = {.fd = fds[0], .events = POLLIN, .revents = 0};
    char buf[256];
    ssize_t total = 0;
    for (int spin = 0; spin < 2000 && poll(&pfd, 1, 10) >= 0; spin++) {
        ssize_t n = read(fds[0], buf + total, sizeof(buf) - 1 - total);
        if (n > 0) {
            total += n;
            if ((size_t)total + 1 >= sizeof(buf)) {
                break;
            }
        } else if (n == 0) {
            break;
        } else if (total > 0) {
            break;
        }
        if (pfd.revents & (POLLHUP | POLLERR)) {
            break;
        }
    }
    close(fds[0]);
    buf[total] = 0;
    log_line("pasted=%s", buf);
}

static void device_enter(void *d, struct wl_data_device *dev, uint32_t serial,
                         struct wl_surface *s, wl_fixed_t x, wl_fixed_t y,
                         struct wl_data_offer *o) {
    (void)d; (void)dev; (void)serial; (void)s; (void)x; (void)y; (void)o;
}
static void device_leave(void *d, struct wl_data_device *dev) {
    (void)d; (void)dev;
}
static void device_motion(void *d, struct wl_data_device *dev, uint32_t t, wl_fixed_t x,
                          wl_fixed_t y) {
    (void)d; (void)dev; (void)t; (void)x; (void)y;
}
static void device_drop(void *d, struct wl_data_device *dev) {
    (void)d; (void)dev;
}

static const struct wl_data_device_listener device_listener = {
    .data_offer = device_data_offer,
    .selection = device_selection,
    .enter = device_enter,
    .leave = device_leave,
    .motion = device_motion,
    .drop = device_drop,
};

int main(int argc, char **argv) {
    if (argc > 1) {
        g_mode = argv[1];
    }
    g_display = wl_display_connect(NULL);
    if (!g_display) {
        return 1;
    }
    struct wl_registry *registry = wl_display_get_registry(g_display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    wl_display_roundtrip(g_display);
    if (!g_compositor || !g_shm || !g_seat || !g_ddm || !g_wm_base) {
        return 2;
    }
    log_line("probe-ready");

    g_data_device = wl_data_device_manager_get_data_device(g_ddm, g_seat);
    if (strcmp(g_mode, "paste") == 0) {
        wl_data_device_add_listener(g_data_device, &device_listener, NULL);
    }

    g_surface = wl_compositor_create_surface(g_compositor);
    g_xdg_surface = xdg_wm_base_get_xdg_surface(g_wm_base, g_surface);
    xdg_surface_add_listener(g_xdg_surface, &xdg_listener, NULL);
    g_toplevel = xdg_surface_get_toplevel(g_xdg_surface);
    xdg_toplevel_set_title(g_toplevel, "clipboard-probe");
    xdg_toplevel_set_app_id(g_toplevel, "dev.tessera.clipboard-probe");
    wl_surface_commit(g_surface);

    // Wait for map + keyboard focus.
    for (int i = 0; i < 200 && (!g_mapped || !g_focused); i++) {
        wl_display_dispatch(g_display);
    }
    if (!g_mapped || !g_focused) {
        log_line("probe-failed mapped=%d focused=%d", g_mapped, g_focused);
        return 3;
    }

    if (strcmp(g_mode, "paste") != 0) {
        struct wl_data_source *source = wl_data_device_manager_create_data_source(g_ddm);
        wl_data_source_add_listener(source, &source_listener, NULL);
        wl_data_source_offer(source, "text/plain");
        wl_data_device_set_selection(g_data_device, source, g_enter_serial);
        wl_display_flush(g_display);
        g_selection_set = 1;
        log_line("selection-set");

        // Serve sends (from wl-paste) and stay alive until killed.
        while (wl_display_dispatch(g_display) >= 0) {
        }
        return 0;
    }

    // paste mode: the test runs wl-copy first; the compositor advertises the
    // selection to this focused client on device bind, so a roundtrip is
    // enough to observe it. Then keep dispatching while the pipe drains.
    wl_display_roundtrip(g_display);
    for (int i = 0; i < 400; i++) {
        wl_display_dispatch(g_display);
        usleep(1000);
    }
    wl_display_flush(g_display);
    return 0;
}
