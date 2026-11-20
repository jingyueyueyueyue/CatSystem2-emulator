#include "egl_presenter.h"

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <android/log.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define PRESENT_TAG "CS2Gpu"
#define DISPLAY_HISTORY 32

typedef struct {
    EGLuint64KHR id;
    uint64_t submitted_ns;
} PendingFrame;

typedef struct {
    PFNEGLGETNEXTFRAMEIDANDROIDPROC next_frame_id;
    PFNEGLGETFRAMETIMESTAMPSANDROIDPROC timestamps;
    PendingFrame frames[DISPLAY_HISTORY];
    size_t first;
    size_t pending;
    uint64_t displayed;
    uint64_t latest_ns;
    uint64_t queue_to_display_ns;
    uint64_t unresolved;
    bool has_queue_time;
    bool enabled;
} FrameTimestamps;

typedef struct {
    ANativeWindow *window;
    EGLDisplay display;
    EGLSurface surface;
    EGLContext context;
    GLuint texture;
    uint8_t *pixels;
    int width;
    int height;
    bool unavailable;
    bool presented;
    FrameTimestamps stats;
} Presenter;

static _Thread_local Presenter presenter;
static _Thread_local bool stats_enabled;
static _Thread_local uint64_t stats_generation;

static uint64_t monotonic_ns(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return 0;
    return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}

static bool has_extension(const char *extensions, const char *name) {
    if (extensions == NULL) return false;
    const size_t length = strlen(name);
    for (const char *match = strstr(extensions, name); match != NULL; match = strstr(match + length, name)) {
        if ((match == extensions || match[-1] == ' ') && (match[length] == ' ' || match[length] == '\0')) return true;
    }
    return false;
}

static void configure_frame_timestamps(void) {
    if (!stats_enabled || presenter.surface == EGL_NO_SURFACE) return;
    ++stats_generation;
    const EGLDisplay display = presenter.display;
    const EGLSurface surface = presenter.surface;
    if (!has_extension(eglQueryString(display, EGL_EXTENSIONS), "EGL_ANDROID_get_frame_timestamps")) return;
    const PFNEGLGETNEXTFRAMEIDANDROIDPROC next =
        (PFNEGLGETNEXTFRAMEIDANDROIDPROC)eglGetProcAddress("eglGetNextFrameIdANDROID");
    const PFNEGLGETFRAMETIMESTAMPSANDROIDPROC timestamps =
        (PFNEGLGETFRAMETIMESTAMPSANDROIDPROC)eglGetProcAddress("eglGetFrameTimestampsANDROID");
    const PFNEGLGETFRAMETIMESTAMPSUPPORTEDANDROIDPROC supported =
        (PFNEGLGETFRAMETIMESTAMPSUPPORTEDANDROIDPROC)eglGetProcAddress("eglGetFrameTimestampSupportedANDROID");
    if (next == NULL || timestamps == NULL || supported == NULL) return;
    if (!supported(display, surface, EGL_DISPLAY_PRESENT_TIME_ANDROID)
        || !eglSurfaceAttrib(display, surface, EGL_TIMESTAMPS_ANDROID, EGL_TRUE)) {
        eglGetError();
        return;
    }
    presenter.stats.next_frame_id = next;
    presenter.stats.timestamps = timestamps;
    presenter.stats.enabled = true;
    presenter.stats.has_queue_time = supported(display, surface, EGL_REQUESTED_PRESENT_TIME_ANDROID);
}

static void remove_frame(FrameTimestamps *stats) {
    stats->first = (stats->first + 1) % DISPLAY_HISTORY;
    --stats->pending;
}

static void poll_frame_timestamps(size_t budget) {
    FrameTimestamps *stats = &presenter.stats;
    if (stats->timestamps == NULL || stats->pending == 0) return;
    const EGLint names[] = { EGL_DISPLAY_PRESENT_TIME_ANDROID, EGL_REQUESTED_PRESENT_TIME_ANDROID };
    const uint64_t now = monotonic_ns();
    // EGL returns pending rather than waiting on display fences. Keep the
    // per-swap query budget small and expire IDs that never become available.
    while (stats->pending != 0 && budget-- != 0) {
        const PendingFrame frame = stats->frames[stats->first];
        EGLnsecsANDROID values[2] = {0};
        if (!stats->timestamps(presenter.display, presenter.surface, frame.id,
                              stats->has_queue_time ? 2 : 1, names, values)) {
            const EGLint error = eglGetError();
            if (error == EGL_BAD_SURFACE || error == EGL_BAD_PARAMETER) {
                stats->unresolved += stats->pending;
                stats->pending = 0;
                stats->timestamps = NULL;
                return;
            }
            ++stats->unresolved;
        } else if (values[0] == EGL_TIMESTAMP_PENDING_ANDROID && now >= frame.submitted_ns
                   && now - frame.submitted_ns < UINT64_C(500000000)) {
            break;
        } else if (values[0] > 0 && (uint64_t)values[0] > stats->latest_ns) {
            ++stats->displayed;
            stats->latest_ns = (uint64_t)values[0];
            stats->queue_to_display_ns = values[1] > 0 && values[0] >= values[1]
                ? (uint64_t)(values[0] - values[1]) : 0;
        } else {
            ++stats->unresolved;
        }
        remove_frame(stats);
    }
}

void cs2_egl_set_stats_enabled(bool enabled) {
    if (stats_enabled == enabled) return;
    stats_enabled = enabled;
    if (presenter.stats.enabled && presenter.surface != EGL_NO_SURFACE) {
        if (!eglSurfaceAttrib(presenter.display, presenter.surface, EGL_TIMESTAMPS_ANDROID, EGL_FALSE)) eglGetError();
    }
    presenter.stats = (FrameTimestamps){0};
    configure_frame_timestamps();
}

Cs2DisplayStats cs2_egl_display_stats(void) {
    poll_frame_timestamps(8);
    const FrameTimestamps *stats = &presenter.stats;
    enum Cs2DisplaySource source = CS2_DISPLAY_NONE;
    if (presenter.unavailable) source = CS2_DISPLAY_SOFTWARE;
    else if (presenter.surface != EGL_NO_SURFACE) {
        if (stats->timestamps == NULL) source = CS2_DISPLAY_EGL_UNSUPPORTED;
        else source = stats->displayed ? CS2_DISPLAY_EGL_TIMESTAMP : CS2_DISPLAY_EGL_WAITING;
    }
    return (Cs2DisplayStats){ source, stats_generation, stats->displayed, stats->latest_ns,
        stats->queue_to_display_ns, stats->unresolved, stats->pending };
}

static bool egl_error(const char *operation) {
    __android_log_print(ANDROID_LOG_WARN, PRESENT_TAG, "%s failed (EGL 0x%x)",
                        operation, (unsigned int)eglGetError());
    return false;
}

static bool gl_ok(const char *operation) {
    const GLenum error = glGetError();
    if (error == GL_NO_ERROR) return true;
    __android_log_print(ANDROID_LOG_WARN, PRESENT_TAG, "%s failed (GL 0x%x)", operation, error);
    return false;
}

static bool disconnect(void) {
    if (presenter.display == EGL_NO_DISPLAY) return true;
    if (presenter.context != EGL_NO_CONTEXT && eglGetCurrentContext() == presenter.context
        && !eglMakeCurrent(presenter.display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT)
        && !eglReleaseThread()) {
        return egl_error("Unbinding the player EGL context");
    }
    if (presenter.surface != EGL_NO_SURFACE && !eglDestroySurface(presenter.display, presenter.surface)) {
        const EGLint error = eglGetError();
        if (error != EGL_BAD_SURFACE) {
            __android_log_print(ANDROID_LOG_WARN, PRESENT_TAG, "Destroying the player EGL surface failed (EGL 0x%x)",
                                (unsigned int)error);
            return false;
        }
    }
    presenter.surface = EGL_NO_SURFACE;
    presenter.stats = (FrameTimestamps){0};
    if (presenter.context != EGL_NO_CONTEXT && !eglDestroyContext(presenter.display, presenter.context)) {
        egl_error("Destroying the player EGL context");
    }
    presenter.context = EGL_NO_CONTEXT;
    if (!eglTerminate(presenter.display)) egl_error("Releasing the player EGL display");
    presenter.display = EGL_NO_DISPLAY;
    return true;
}

bool cs2_egl_close(void) {
    if (!disconnect()) return false;
    if (presenter.window != NULL) ANativeWindow_release(presenter.window);
    free(presenter.pixels);
    presenter = (Presenter){0};
    return true;
}

enum Cs2EglResult cs2_egl_disable(void) {
    presenter.unavailable = true;
    if (!disconnect()) return CS2_EGL_RETRY;
    free(presenter.pixels);
    presenter.pixels = NULL;
    __android_log_write(ANDROID_LOG_WARN, PRESENT_TAG, "present=software");
    return CS2_EGL_SOFTWARE;
}

static GLuint compile_shader(GLenum type, const char *source) {
    GLuint shader = glCreateShader(type);
    if (shader == 0) {
        gl_ok("Creating the presentation shader");
        return 0;
    }
    GLint compiled = 0;
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    glGetShaderiv(shader, GL_COMPILE_STATUS, &compiled);
    if (!compiled) {
        char log[512] = {0};
        glGetShaderInfoLog(shader, sizeof(log), NULL, log);
        __android_log_print(ANDROID_LOG_WARN, PRESENT_TAG, "Presentation shader compilation failed: %s", log);
        glDeleteShader(shader);
        return 0;
    }
    return shader;
}

static bool create_program(void) {
    // CPU row zero is the top of the game canvas. The draw below gives the
    // top edge texture coordinate 0 and the bottom edge texture coordinate 1.
    const char *vertex =
        "attribute vec2 position; attribute vec2 uv; varying highp vec2 coord;"
        "void main() { gl_Position = vec4(position, 0.0, 1.0); coord = uv; }";
    const char *fragment =
        "precision highp float; varying highp vec2 coord; uniform sampler2D pixels;"
        "void main() { gl_FragColor = texture2D(pixels, coord); }";
    const GLuint vs = compile_shader(GL_VERTEX_SHADER, vertex);
    if (vs == 0) return false;
    const GLuint fs = compile_shader(GL_FRAGMENT_SHADER, fragment);
    if (fs == 0) {
        glDeleteShader(vs);
        return false;
    }
    const GLuint program = glCreateProgram();
    if (program == 0) {
        glDeleteShader(vs);
        glDeleteShader(fs);
        gl_ok("Creating the presentation program");
        return false;
    }
    glAttachShader(program, vs);
    glAttachShader(program, fs);
    glBindAttribLocation(program, 0, "position");
    glBindAttribLocation(program, 1, "uv");
    glLinkProgram(program);
    glDeleteShader(vs);
    glDeleteShader(fs);
    GLint linked = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &linked);
    if (!linked) {
        char log[512] = {0};
        glGetProgramInfoLog(program, sizeof(log), NULL, log);
        __android_log_print(ANDROID_LOG_WARN, PRESENT_TAG, "Presentation program link failed: %s", log);
        glDeleteProgram(program);
        return false;
    }
    glUseProgram(program);
    const GLint sampler = glGetUniformLocation(program, "pixels");
    if (sampler >= 0) glUniform1i(sampler, 0);
    glDeleteProgram(program);
    return gl_ok("Setting up the presentation program") && sampler >= 0;
}

static bool connect(void) {
    const EGLint config_attributes[] = {
        EGL_SURFACE_TYPE, EGL_WINDOW_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8,
        EGL_NONE,
    };
    const EGLint context_attributes[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
    EGLDisplay display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (display == EGL_NO_DISPLAY || !eglInitialize(display, NULL, NULL)) {
        return egl_error("Initializing EGL for presentation");
    }
    presenter.display = display;
    EGLConfig config;
    EGLint count = 0;
    if (!eglChooseConfig(display, config_attributes, &config, 1, &count)) {
        return egl_error("Choosing the EGL presentation configuration");
    }
    if (count == 0) {
        __android_log_write(ANDROID_LOG_WARN, PRESENT_TAG, "EGL has no RGBA8 window configuration");
        return false;
    }
    EGLint format;
    if (!eglGetConfigAttrib(display, config, EGL_NATIVE_VISUAL_ID, &format)) {
        return egl_error("Reading the EGL window format");
    }
    if (format != WINDOW_FORMAT_RGBA_8888) {
        __android_log_print(ANDROID_LOG_WARN, PRESENT_TAG, "EGL window format is %d, expected RGBA8888", format);
        return false;
    }
    if (ANativeWindow_setBuffersGeometry(presenter.window, presenter.width, presenter.height, format) != 0) {
        __android_log_write(ANDROID_LOG_WARN, PRESENT_TAG, "Setting the EGL buffer geometry failed");
        return false;
    }
    presenter.surface = eglCreateWindowSurface(display, config, presenter.window, NULL);
    if (presenter.surface == EGL_NO_SURFACE) return egl_error("Creating the EGL window surface");
    if (!eglBindAPI(EGL_OPENGL_ES_API)) return egl_error("Selecting OpenGL ES for presentation");
    presenter.context = eglCreateContext(display, config, EGL_NO_CONTEXT, context_attributes);
    if (presenter.context == EGL_NO_CONTEXT) return egl_error("Creating the OpenGL ES context");
    if (!eglMakeCurrent(display, presenter.surface, presenter.surface, presenter.context)) {
        return egl_error("Binding the OpenGL ES presentation context");
    }
    if (!create_program()) return false;
    glDisable(GL_DITHER);
    glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
    glActiveTexture(GL_TEXTURE0);
    glGenTextures(1, &presenter.texture);
    glBindTexture(GL_TEXTURE_2D, presenter.texture);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, presenter.width, presenter.height, 0,
                 GL_RGBA, GL_UNSIGNED_BYTE, NULL);
    if (!gl_ok("Allocating the presentation texture")) return false;
    configure_frame_timestamps();
    return true;
}

enum Cs2EglResult cs2_egl_prepare(ANativeWindow *window, int width, int height) {
    if (presenter.window == window && presenter.width == width && presenter.height == height) {
        if (!presenter.unavailable) return CS2_EGL_READY;
        return presenter.display == EGL_NO_DISPLAY ? CS2_EGL_SOFTWARE : cs2_egl_disable();
    }
    if (!cs2_egl_close()) return CS2_EGL_RETRY;
    const uint64_t bytes = (uint64_t)width * (uint64_t)height * 4;
    if (width <= 0 || height <= 0 || bytes > 512 * UINT64_C(1024) * 1024) return CS2_EGL_SOFTWARE;
    ANativeWindow_acquire(window);
    presenter.window = window;
    presenter.width = width;
    presenter.height = height;
    presenter.pixels = malloc((size_t)bytes);
    if (presenter.pixels == NULL || !connect()) return cs2_egl_disable();
    return CS2_EGL_READY;
}

uint8_t *cs2_egl_pixels(void) {
    return presenter.pixels;
}

bool cs2_egl_draw(void) {
    static const GLfloat vertices[][4] = {
        {-1.0f,  1.0f, 0.0f, 0.0f},
        {-1.0f, -1.0f, 0.0f, 1.0f},
        { 1.0f,  1.0f, 1.0f, 0.0f},
        { 1.0f, -1.0f, 1.0f, 1.0f},
    };
    EGLint width, height;
    if (!eglQuerySurface(presenter.display, presenter.surface, EGL_WIDTH, &width)
        || !eglQuerySurface(presenter.display, presenter.surface, EGL_HEIGHT, &height)) {
        return egl_error("Measuring the EGL surface");
    }
    if (width <= 0 || height <= 0) return false;
    glViewport(0, 0, width, height);
    glTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, presenter.width, presenter.height,
                    GL_RGBA, GL_UNSIGNED_BYTE, presenter.pixels);
    glEnableVertexAttribArray(0);
    glEnableVertexAttribArray(1);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, sizeof(vertices[0]), &vertices[0][0]);
    glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, sizeof(vertices[0]), &vertices[0][2]);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    return gl_ok("Drawing the presentation frame");
}

bool cs2_egl_swap(void) {
    FrameTimestamps *stats = &presenter.stats;
    EGLuint64KHR frame_id = 0;
    bool track_frame = false;
    if (stats->timestamps != NULL) {
        poll_frame_timestamps(4);
        if (stats->timestamps != NULL) {
            track_frame = stats->next_frame_id(presenter.display, presenter.surface, &frame_id);
            if (!track_frame) { eglGetError(); ++stats->unresolved; }
        }
    }
    if (!eglSwapBuffers(presenter.display, presenter.surface)) return egl_error("Swapping the EGL window buffers");
    if (track_frame) {
        if (stats->pending == DISPLAY_HISTORY) { ++stats->unresolved; remove_frame(stats); }
        stats->frames[(stats->first + stats->pending++) % DISPLAY_HISTORY] = (PendingFrame){ frame_id, monotonic_ns() };
    }
    if (!presenter.presented) {
        presenter.presented = true;
        __android_log_write(ANDROID_LOG_INFO, PRESENT_TAG, "present=egl_gles2");
    }
    return true;
}
