#ifndef CS2_EGL_PRESENTER_H
#define CS2_EGL_PRESENTER_H

#include <android/native_window.h>
#include <stdbool.h>
#include <stdint.h>

enum Cs2EglResult {
    CS2_EGL_READY,
    CS2_EGL_SOFTWARE,
    CS2_EGL_RETRY,
};

enum Cs2DisplaySource {
    CS2_DISPLAY_NONE,
    CS2_DISPLAY_SOFTWARE,
    CS2_DISPLAY_EGL_UNSUPPORTED,
    CS2_DISPLAY_EGL_WAITING,
    CS2_DISPLAY_EGL_TIMESTAMP,
    CS2_DISPLAY_VULKAN_UNSUPPORTED,
    CS2_DISPLAY_VULKAN_WAITING,
    CS2_DISPLAY_VULKAN_TIMESTAMP,
};

typedef struct {
    enum Cs2DisplaySource source;
    uint64_t generation;
    uint64_t displayed;
    uint64_t latest_ns;
    uint64_t queue_to_display_ns;
    uint64_t unresolved;
    uint64_t pending;
} Cs2DisplayStats;

// The player worker owns the EGL context and calls these on the same thread.
enum Cs2EglResult cs2_egl_prepare(ANativeWindow *window, int width, int height);
uint8_t *cs2_egl_pixels(void);
bool cs2_egl_draw(void);
bool cs2_egl_swap(void);
enum Cs2EglResult cs2_egl_disable(void);
bool cs2_egl_close(void);
void cs2_egl_set_stats_enabled(bool enabled);
Cs2DisplayStats cs2_egl_display_stats(void);

#endif
