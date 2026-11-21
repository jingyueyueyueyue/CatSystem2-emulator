#include <jni.h>
#include <android/log.h>
#include <android/native_window.h>
#include <android/native_window_jni.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/system_properties.h>
#include <time.h>

#include "egl_presenter.h"

// Keep these indices aligned with HostPhase in the portable Rust host.
enum {
    PERF_COMPOSE_OTHER = 4, PERF_SCRIPT_DETAIL = 5, PERF_SCRIPT_READ = 13, PERF_SCRIPT_PARSE,
    PERF_OPEN_SCAN, PERF_OPEN_INITIALIZE, PERF_OPEN_STARTUP,
    PERF_COMPOSE_LAYERS_OTHER, PERF_COMPOSE_TEXT, PERF_COMPOSE_EMOTE, PERF_COMPOSE_TRANSITION,
    PERF_COMPOSE_PLANE, PERF_COMPOSE_KX, PERF_COMPOSE_RESOURCE_REDRAW, PERF_PHASES,
    PERF_BATCH = 8
};
enum { FRAME_CHANGED = 1, FRAME_UNCHANGED = 4 };

typedef struct {
    uint64_t phase_ns[PERF_PHASES];
} HostFrameTiming;

extern uint64_t cs2_android_open_profiled_v3(const char *, const char *, int32_t, HostFrameTiming *);
extern int32_t cs2_android_close(uint64_t);
extern int32_t cs2_android_step_profiled_v4(uint64_t, uint32_t, HostFrameTiming *, const char **,
                                           uint64_t, uint64_t, const char **);
extern int32_t cs2_android_size(uint64_t, uint32_t);
extern int32_t cs2_android_take_pointer(uint64_t, int32_t *);
extern int32_t cs2_android_take_cursor_visibility(uint64_t);
extern int32_t cs2_android_take_window_request(uint64_t);
extern int32_t cs2_android_general_window_policy(uint64_t);
extern int32_t cs2_android_general_window_caption(uint64_t, const uint8_t **, size_t *);
extern int32_t cs2_android_window_mode(uint64_t, int32_t);
extern int32_t cs2_android_window_ui(uint64_t, const uint8_t **, size_t *);
extern int32_t cs2_android_window_ui_action(uint64_t, uint64_t, uint32_t, uint32_t, uint32_t, uint32_t);
extern int32_t cs2_android_settings_apply(uint64_t, uint64_t, const uint8_t *, size_t);
extern int32_t cs2_android_settings_font_preview(uint64_t, uint64_t, uint32_t, uint32_t, const uint8_t **, size_t *);
extern const char *cs2_android_take_clipboard(uint64_t);
extern int32_t cs2_android_take_clipboard_wide(uint64_t, const uint16_t **, size_t *);
extern int32_t cs2_android_message_lists(uint64_t, const uint8_t **, size_t *);
extern int32_t cs2_android_message_list_action(uint64_t, uint32_t, uint32_t, uint32_t);
extern int32_t cs2_android_scene_tools(uint64_t, const uint8_t **, size_t *);
extern int32_t cs2_android_scene_tool_action(uint64_t, uint32_t, uint32_t, uint32_t, uint32_t);
extern int32_t cs2_android_system_open(uint64_t, const uint8_t **, size_t *);
extern int32_t cs2_android_system_open_result(uint64_t, uint64_t, const uint8_t *, size_t);
extern int32_t cs2_android_tweet(uint64_t, const uint8_t **, size_t *);
extern int32_t cs2_android_tweet_result(uint64_t, uint64_t, uint32_t, uint32_t, const uint8_t *, size_t);
extern int32_t cs2_android_tweet_cancel(uint64_t);
extern int32_t cs2_android_tweet_count(const uint8_t *, size_t, const uint8_t *, size_t);
extern int32_t cs2_android_dialog(uint64_t, const uint8_t **, size_t *);
extern int32_t cs2_android_dialog_result(uint64_t, uint64_t, int32_t);
extern int32_t cs2_android_input(uint64_t, uint32_t, int32_t, int32_t);
extern int32_t cs2_android_text(uint64_t, uint32_t, const uint8_t *, size_t, int32_t, int32_t);
extern int32_t cs2_android_copy_frame(uint64_t, uint8_t *, int32_t, int32_t, int32_t);
extern int32_t cs2_android_frame_posted(uint64_t);
extern int32_t cs2_android_gpu_timing(uint64_t *, uint64_t *);
extern int32_t cs2_android_vulkan_candidate(void *);
extern int32_t cs2_android_present_vulkan(uint64_t, void *);
extern void cs2_android_detach_vulkan(void);
extern void cs2_android_vulkan_display_stats_enable(int32_t);
extern int32_t cs2_android_vulkan_display_stats(uint64_t *);
extern const char *cs2_android_last_error(void);

#define JNI(name) Java_org_catsystem2_player_PlayerActivity_##name
#define UNUSED_TYPES (void)type
#define PERF_TAG "CS2Perf"
#define STORY_TAG "CS2Story"

typedef struct {
    uint64_t frame;
    uint64_t step_calls;
    uint64_t first_step_ns;
    uint64_t step_ns;
    HostFrameTiming timing;
} PerfTick;

typedef struct {
    uint64_t handle;
    uint64_t opened_from_ns;
    uint64_t last_post_ns;
    uint64_t post_sequence;
    uint64_t frame_sequence;
    PerfTick tick;
    size_t log_size;
    size_t log_rows;
    char log_batch[4000];
} PerfSession;

static _Thread_local PerfSession perf;

typedef struct {
    uint64_t tick_work_ns;
    uint64_t last_tick_ns;
    uint64_t last_tick_at_ns;
    uint64_t last_submit_at_ns;
    uint64_t present_ns;
    uint64_t copy_ns;
    uint64_t swap_ns;
} HudTimings;

static _Thread_local bool hud_enabled;
static _Thread_local HudTimings hud;

static uint64_t perf_now_ns(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}

static int perf_enabled(void) {
    char value[PROP_VALUE_MAX];
    return __system_property_get("log.tag." PERF_TAG, value) == 1 && value[0] == 'D';
}

static void perf_flush(void) {
    if (perf.log_rows == 0) return;
    __android_log_write(ANDROID_LOG_INFO, PERF_TAG, perf.log_batch);
    perf.log_size = 0;
    perf.log_rows = 0;
}

static void perf_append_row(const char *row, size_t length) {
    if (perf.log_size + length >= sizeof(perf.log_batch)) perf_flush();
    memcpy(perf.log_batch + perf.log_size, row, length + 1);
    perf.log_size += length;
    perf.log_rows++;
    if (perf.log_rows == PERF_BATCH) perf_flush();
}

static uint64_t perf_script_ns(const uint64_t *phases) {
    uint64_t script = phases[0];
    for (size_t detail = PERF_SCRIPT_DETAIL; detail < PERF_SCRIPT_READ; detail++) script += phases[detail];
    return script;
}

static uint64_t perf_layers_ns(const uint64_t *phase) {
    return phase[PERF_COMPOSE_LAYERS_OTHER] + phase[PERF_COMPOSE_PLANE]
        + phase[PERF_COMPOSE_KX] + phase[PERF_COMPOSE_RESOURCE_REDRAW];
}

static uint64_t perf_compose_ns(const uint64_t *phase) {
    return phase[PERF_COMPOSE_OTHER] + perf_layers_ns(phase) + phase[PERF_COMPOSE_TEXT]
        + phase[PERF_COMPOSE_EMOTE] + phase[PERF_COMPOSE_TRANSITION];
}

static void perf_record_unchanged(void) {
    char row[1360];
    const uint64_t *phase = perf.tick.timing.phase_ns;
    const int length = snprintf(row, sizeof(row), "unchanged session=%" PRIu64
        " frame=%" PRIu64 " step_us=%" PRIu64 " compose_us=%" PRIu64 " step_calls=%" PRIu64
        " script_us=%" PRIu64 " script_read_us=%" PRIu64 " script_parse_us=%" PRIu64
        " image_read_us=%" PRIu64 " image_decode_us=%" PRIu64 " image_patch_us=%" PRIu64
        " compose_layers_us=%" PRIu64 " compose_text_us=%" PRIu64 " compose_emote_us=%" PRIu64
        " compose_transition_us=%" PRIu64 " compose_other_us=%" PRIu64
        " compose_plane_us=%" PRIu64 " compose_kx_us=%" PRIu64 " compose_resource_redraw_us=%" PRIu64
        " compose_layers_other_us=%" PRIu64
        " script_input_us=%" PRIu64 " script_root_us=%" PRIu64 " script_children_us=%" PRIu64
        " script_fes_us=%" PRIu64 " script_list_us=%" PRIu64 " script_list_raster_us=%" PRIu64
        " script_motion_raster_us=%" PRIu64 " script_button_us=%" PRIu64 "\n",
        perf.handle, perf.tick.frame, perf.tick.step_ns / 1000, perf_compose_ns(phase) / 1000, perf.tick.step_calls,
        perf_script_ns(phase) / 1000, phase[PERF_SCRIPT_READ] / 1000, phase[PERF_SCRIPT_PARSE] / 1000,
        phase[1] / 1000, phase[2] / 1000, phase[3] / 1000,
        perf_layers_ns(phase) / 1000, phase[PERF_COMPOSE_TEXT] / 1000, phase[PERF_COMPOSE_EMOTE] / 1000,
        phase[PERF_COMPOSE_TRANSITION] / 1000, phase[PERF_COMPOSE_OTHER] / 1000,
        phase[PERF_COMPOSE_PLANE] / 1000, phase[PERF_COMPOSE_KX] / 1000,
        phase[PERF_COMPOSE_RESOURCE_REDRAW] / 1000, phase[PERF_COMPOSE_LAYERS_OTHER] / 1000,
        phase[5] / 1000, phase[6] / 1000, phase[7] / 1000, phase[8] / 1000,
        phase[9] / 1000, phase[10] / 1000, phase[11] / 1000, phase[12] / 1000);
    if (length > 0 && (size_t)length < sizeof(row)) perf_append_row(row, (size_t)length);
    perf.tick = (PerfTick){0};
}

static void perf_record_post(uint64_t present_start, uint64_t lock_start, uint64_t copy_start,
                             uint64_t post_start, uint64_t posted_at, uint64_t present_end) {
    const PerfTick no_completed_tick = {0};
    const PerfTick *tick = perf.tick.frame ? &perf.tick : &no_completed_tick;
    const uint64_t interval = perf.last_post_ns ? (posted_at - perf.last_post_ns) / 1000 : 0;
    perf.last_post_ns = posted_at;
    perf.post_sequence++;
    if (perf.post_sequence == 1) {
        __android_log_print(ANDROID_LOG_INFO, PERF_TAG, "first_post session=%" PRIu64
                            " open_to_post_us=%" PRIu64, perf.handle,
                            (posted_at - perf.opened_from_ns) / 1000);
    }
    char row[768];
    const uint64_t *phase = tick->timing.phase_ns;
    const int length = snprintf(row, sizeof(row), "f,%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 "\n",
        perf.handle, perf.post_sequence, tick->frame, tick->step_calls, tick->step_ns / 1000,
        perf_script_ns(phase) / 1000, phase[1] / 1000, phase[2] / 1000, phase[3] / 1000, perf_compose_ns(phase) / 1000,
        (present_end - present_start) / 1000, (copy_start - lock_start) / 1000,
        (post_start - copy_start) / 1000, (posted_at - post_start) / 1000, interval,
        posted_at / 1000, tick->first_step_ns ? (posted_at - tick->first_step_ns) / 1000 : 0,
        phase[5] / 1000, phase[6] / 1000, phase[7] / 1000, phase[8] / 1000,
        phase[9] / 1000, phase[10] / 1000, phase[11] / 1000, phase[12] / 1000,
        phase[PERF_SCRIPT_READ] / 1000, phase[PERF_SCRIPT_PARSE] / 1000,
        perf_layers_ns(phase) / 1000, phase[PERF_COMPOSE_TEXT] / 1000, phase[PERF_COMPOSE_EMOTE] / 1000,
        phase[PERF_COMPOSE_TRANSITION] / 1000, phase[PERF_COMPOSE_OTHER] / 1000,
        phase[PERF_COMPOSE_PLANE] / 1000, phase[PERF_COMPOSE_KX] / 1000,
        phase[PERF_COMPOSE_RESOURCE_REDRAW] / 1000, phase[PERF_COMPOSE_LAYERS_OTHER] / 1000);
    if (length > 0 && (size_t)length < sizeof(row)) {
        perf_append_row(row, (size_t)length);
        if (perf.post_sequence == 1) perf_flush();
    }
    if (perf.tick.frame) perf.tick = (PerfTick){0};
}

static char *path_bytes(JNIEnv *env, jbyteArray bytes) {
    if (bytes == NULL) return NULL;
    jsize length = (*env)->GetArrayLength(env, bytes);
    if (length < 1 || length > 32768) return NULL;
    char *value = malloc((size_t)length + 1);
    if (value == NULL) return NULL;
    (*env)->GetByteArrayRegion(env, bytes, 0, length, (jbyte *)value);
    if ((*env)->ExceptionCheck(env) || memchr(value, 0, (size_t)length) != NULL) {
        free(value);
        return NULL;
    }
    value[length] = '\0';
    return value;
}

JNIEXPORT jlong JNICALL JNI(nativeOpen)(JNIEnv *env, jclass type, jbyteArray game, jbyteArray saves) {
    UNUSED_TYPES;
    const uint64_t start = perf_enabled() ? perf_now_ns() : 0;
    char story_value[PROP_VALUE_MAX];
    const int story = __system_property_get("log.tag." STORY_TAG, story_value) == 1 && story_value[0] == 'D';
    char *game_path = path_bytes(env, game);
    char *save_path = path_bytes(env, saves);
    uint64_t handle = 0;
    HostFrameTiming timing = {0};
    if (game_path != NULL && save_path != NULL) {
        handle = cs2_android_open_profiled_v3(game_path, save_path, story, start ? &timing : NULL);
    }
    free(game_path);
    free(save_path);
    if (start && handle) {
        perf = (PerfSession){ .handle = handle, .opened_from_ns = start };
        __android_log_print(ANDROID_LOG_INFO, PERF_TAG, "open session=%" PRIu64 " native_us=%" PRIu64,
                            handle, (perf_now_ns() - start) / 1000);
        const uint64_t *phase = timing.phase_ns;
        __android_log_print(ANDROID_LOG_INFO, PERF_TAG, "load session=%" PRIu64
            " scan_us=%" PRIu64 " init_us=%" PRIu64 " startup_us=%" PRIu64
            " script_read_us=%" PRIu64 " script_parse_us=%" PRIu64, handle,
            phase[PERF_OPEN_SCAN] / 1000, phase[PERF_OPEN_INITIALIZE] / 1000, phase[PERF_OPEN_STARTUP] / 1000,
            phase[PERF_SCRIPT_READ] / 1000, phase[PERF_SCRIPT_PARSE] / 1000);
        __android_log_write(ANDROID_LOG_INFO, PERF_TAG,
            "f,session,post,frame,step_calls,step_us,script_us,image_read_us,image_decode_us,"
            "image_patch_us,compose_us,present_us,lock_us,copy_us,post_us,interval_us,post_mono_us,tick_to_post_us,"
            "script_input_us,script_root_us,script_children_us,script_fes_us,script_list_us,script_list_raster_us,"
            "script_motion_raster_us,script_button_us,script_read_us,script_parse_us,"
            "compose_layers_us,compose_text_us,compose_emote_us,compose_transition_us,compose_other_us,"
            "compose_plane_us,compose_kx_us,compose_resource_redraw_us,compose_layers_other_us");
    }
    return (jlong)handle;
}

JNIEXPORT jint JNICALL JNI(nativeClose)(JNIEnv *env, jclass type, jlong handle) {
    (void)env; UNUSED_TYPES;
    cs2_android_detach_vulkan();
    cs2_egl_close();
    hud = (HudTimings){0};
    if (perf.handle && perf.handle == (uint64_t)handle) {
        perf_flush();
        perf.handle = 0;
    }
    return cs2_android_close((uint64_t)handle);
}
JNIEXPORT jint JNICALL JNI(nativeStep)(JNIEnv *env, jclass type, jlong handle, jint budget) {
    (void)env; UNUSED_TYPES;
    const int timed = perf.handle && perf.handle == (uint64_t)handle;
    HostFrameTiming sample = {0};
    const uint64_t start = timed || hud_enabled ? perf_now_ns() : 0;
    const char *story_row = NULL;
    const char *details = NULL;
    const int32_t result = cs2_android_step_profiled_v4((uint64_t)handle, (uint32_t)budget,
        timed ? &sample : NULL, &story_row, perf.frame_sequence + 1, perf.tick.step_calls + 1,
        timed ? &details : NULL);
    const uint64_t finished = start ? perf_now_ns() : 0;
    if (hud_enabled) {
        hud.tick_work_ns += finished - start;
        if (result == FRAME_CHANGED || result == FRAME_UNCHANGED) {
            hud.last_tick_ns = hud.tick_work_ns;
            hud.last_tick_at_ns = finished;
            hud.tick_work_ns = 0;
        }
    }
    if (timed) {
        perf.tick.step_ns += finished - start;
        if (perf.tick.step_calls++ == 0) perf.tick.first_step_ns = start;
        for (size_t phase = 0; phase < PERF_PHASES; phase++) {
            perf.tick.timing.phase_ns[phase] += sample.phase_ns[phase];
        }
        if (details != NULL) {
            const size_t length = strlen(details);
            if (length > 0 && length < sizeof(perf.log_batch)) perf_append_row(details, length);
        }
        if (result == FRAME_CHANGED || result == FRAME_UNCHANGED) {
            perf.tick.frame = ++perf.frame_sequence;
            if (result == FRAME_UNCHANGED) perf_record_unchanged();
        }
    }
    if (story_row != NULL) __android_log_write(ANDROID_LOG_DEBUG, STORY_TAG, story_row);
    return result;
}
JNIEXPORT jint JNICALL JNI(nativeSize)(JNIEnv *env, jclass type, jlong handle, jint axis) {
    (void)env; UNUSED_TYPES;
    return cs2_android_size((uint64_t)handle, (uint32_t)axis);
}
JNIEXPORT jint JNICALL JNI(nativePointer)(JNIEnv *env, jclass type, jlong handle, jintArray output) {
    UNUSED_TYPES;
    if (output == NULL || (*env)->GetArrayLength(env, output) < 2) return -1;
    int32_t point[2];
    const int result = cs2_android_take_pointer((uint64_t)handle, point);
    if (result == 1) {
        const jint values[2] = { (jint)point[0], (jint)point[1] };
        (*env)->SetIntArrayRegion(env, output, 0, 2, values);
        if ((*env)->ExceptionCheck(env)) return -1;
    }
    return result;
}
JNIEXPORT jint JNICALL JNI(nativeCursorVisibility)(JNIEnv *env, jclass type, jlong handle) {
    (void)env; UNUSED_TYPES;
    return cs2_android_take_cursor_visibility((uint64_t)handle);
}
JNIEXPORT jint JNICALL JNI(nativeWindowRequest)(JNIEnv *env, jclass type, jlong handle) {
    (void)env; UNUSED_TYPES;
    return cs2_android_take_window_request((uint64_t)handle);
}
JNIEXPORT jint JNICALL JNI(nativeGeneralWindowPolicy)(JNIEnv *env, jclass type, jlong handle) {
    (void)env; UNUSED_TYPES;
    return cs2_android_general_window_policy((uint64_t)handle);
}
JNIEXPORT jint JNICALL JNI(nativeWindowMode)(JNIEnv *env, jclass type, jlong handle, jint windowed) {
    (void)env; UNUSED_TYPES;
    return cs2_android_window_mode((uint64_t)handle, (int32_t)windowed);
}
JNIEXPORT jcharArray JNICALL JNI(nativeClipboard)(JNIEnv *env, jclass type, jlong handle) {
    UNUSED_TYPES;
    const uint16_t *text = NULL;
    size_t length = 0;
    const int status = cs2_android_take_clipboard_wide((uint64_t)handle, &text, &length);
    if (status < 0) {
        jclass exception = (*env)->FindClass(env, "java/lang/IllegalStateException");
        if (exception != NULL) (*env)->ThrowNew(env, exception, "Clipboard request failed");
        return NULL;
    }
    if (status == 0) return NULL;
    if (text == NULL || length > 32768) return NULL;
    jcharArray result = (*env)->NewCharArray(env, (jsize)length);
    if (result != NULL) {
        (*env)->SetCharArrayRegion(env, result, 0, (jsize)length, (const jchar *)text);
    }
    return result;
}
typedef int32_t (*HostPayloadReader)(uint64_t, const uint8_t **, size_t *);
static jbyteArray host_payload(
    JNIEnv *env, jlong handle, HostPayloadReader reader, const char *failure
) {
    const uint8_t *data = NULL;
    size_t length = 0;
    const int status = reader((uint64_t)handle, &data, &length);
    if (status < 0) {
        jclass exception = (*env)->FindClass(env, "java/lang/IllegalStateException");
        if (exception != NULL) (*env)->ThrowNew(env, exception, failure);
        return NULL;
    }
    if (status == 0) return NULL;
    if (data == NULL || length > 64u * 1024u * 1024u) return NULL;
    jbyteArray result = (*env)->NewByteArray(env, (jsize)length);
    if (result != NULL) (*env)->SetByteArrayRegion(env, result, 0, (jsize)length, (const jbyte *)data);
    return result;
}
JNIEXPORT jbyteArray JNICALL JNI(nativeMessageLists)(JNIEnv *env, jclass type, jlong handle) {
    UNUSED_TYPES;
    return host_payload(env, handle, cs2_android_message_lists, "Message-list snapshot failed");
}
JNIEXPORT jbyteArray JNICALL JNI(nativeWindowUi)(JNIEnv *env, jclass type, jlong handle) {
    UNUSED_TYPES;
    return host_payload(env, handle, cs2_android_window_ui, "Window UI snapshot failed");
}
JNIEXPORT jbyteArray JNICALL JNI(nativeGeneralWindowCaption)(JNIEnv *env, jclass type, jlong handle) {
    UNUSED_TYPES;
    return host_payload(env, handle, cs2_android_general_window_caption, "General window caption failed");
}
JNIEXPORT jint JNICALL JNI(nativeWindowUiAction)(JNIEnv *env, jclass type, jlong handle,
    jlong request, jint action, jint a, jint b, jint c) {
    (void)env; UNUSED_TYPES;
    return cs2_android_window_ui_action((uint64_t)handle, (uint64_t)request,
        (uint32_t)action, (uint32_t)a, (uint32_t)b, (uint32_t)c);
}
JNIEXPORT jint JNICALL JNI(nativeSettingsApply)(JNIEnv *env, jclass type, jlong handle,
    jlong request, jbyteArray record) {
    UNUSED_TYPES;
    if (record == NULL) return cs2_android_settings_apply((uint64_t)handle, (uint64_t)request, NULL, 0);
    jsize length = (*env)->GetArrayLength(env, record);
    if (length > 16384) return cs2_android_settings_apply((uint64_t)handle, (uint64_t)request, NULL, (size_t)length);
    jbyte *bytes = (*env)->GetByteArrayElements(env, record, NULL);
    if (bytes == NULL) return -1;
    int32_t result = cs2_android_settings_apply((uint64_t)handle, (uint64_t)request,
        (const uint8_t *)bytes, (size_t)length);
    (*env)->ReleaseByteArrayElements(env, record, bytes, JNI_ABORT);
    return result;
}
JNIEXPORT jbyteArray JNICALL JNI(nativeSettingsFontPreview)(JNIEnv *env, jclass type, jlong handle,
    jlong request, jint face, jint bold) {
    UNUSED_TYPES;
    const uint8_t *data = NULL;
    size_t length = 0;
    int32_t status = cs2_android_settings_font_preview((uint64_t)handle, (uint64_t)request,
        (uint32_t)face, (uint32_t)bold, &data, &length);
    if (status < 0) {
        jclass exception = (*env)->FindClass(env, "java/lang/IllegalStateException");
        if (exception != NULL) (*env)->ThrowNew(env, exception, "Shared font preview failed");
        return NULL;
    }
    if (data == NULL || length < 8 || length > 4u * 1024u * 1024u) return NULL;
    jbyteArray result = (*env)->NewByteArray(env, (jsize)length);
    if (result != NULL) (*env)->SetByteArrayRegion(env, result, 0, (jsize)length, (const jbyte *)data);
    return result;
}
JNIEXPORT jbyteArray JNICALL JNI(nativeSceneTools)(JNIEnv *env, jclass type, jlong handle) {
    UNUSED_TYPES;
    return host_payload(env, handle, cs2_android_scene_tools, "Scene-tool snapshot failed");
}
JNIEXPORT jint JNICALL JNI(nativeSceneToolAction)(
    JNIEnv *env, jclass type, jlong handle, jint task, jint operation, jint parameter, jint value
) {
    (void)env; UNUSED_TYPES;
    return cs2_android_scene_tool_action((uint64_t)handle, (uint32_t)task,
        (uint32_t)operation, (uint32_t)parameter, (uint32_t)value);
}
JNIEXPORT jint JNICALL JNI(nativeMessageListAction)(
    JNIEnv *env, jclass type, jlong handle, jint task, jint operation, jint value
) {
    (void)env; UNUSED_TYPES;
    return cs2_android_message_list_action((uint64_t)handle, (uint32_t)task,
        (uint32_t)operation, (uint32_t)value);
}
JNIEXPORT jbyteArray JNICALL JNI(nativeSystemOpen)(JNIEnv *env, jclass type, jlong handle) {
    UNUSED_TYPES;
    return host_payload(env, handle, cs2_android_system_open, "System-open request failed");
}
JNIEXPORT jbyteArray JNICALL Java_org_catsystem2_player_AndroidTweetHost_nativeTake(
    JNIEnv *env, jclass type, jlong handle
) {
    UNUSED_TYPES;
    const uint8_t *bytes = NULL;
    size_t length = 0;
    int status = cs2_android_tweet((uint64_t)handle, &bytes, &length);
    if (status == 0) return NULL;
    if (status < 0 || bytes == NULL || length > 66u * 1024u * 1024u) {
        jclass exception = (*env)->FindClass(env, "java/lang/IllegalStateException");
        if (exception != NULL) (*env)->ThrowNew(env, exception, "Tweet host request failed or exceeded its limit");
        return NULL;
    }
    jbyteArray result = (*env)->NewByteArray(env, (jsize)length);
    if (result != NULL) (*env)->SetByteArrayRegion(env, result, 0, (jsize)length, (const jbyte *)bytes);
    return result;
}
JNIEXPORT jint JNICALL Java_org_catsystem2_player_AndroidTweetHost_nativeComplete(
    JNIEnv *env, jclass type, jlong handle, jlong request, jint outcome, jint value, jbyteArray receipt
) {
    UNUSED_TYPES;
    if (receipt == NULL) return -1;
    jsize length = (*env)->GetArrayLength(env, receipt);
    if (length < 0 || length > 1024 * 1024) return -1;
    if (length == 0) return cs2_android_tweet_result((uint64_t)handle, (uint64_t)request,
        (uint32_t)outcome, (uint32_t)value, NULL, 0);
    jbyte *bytes = (*env)->GetByteArrayElements(env, receipt, NULL);
    if (bytes == NULL) return -1;
    int result = cs2_android_tweet_result((uint64_t)handle, (uint64_t)request,
        (uint32_t)outcome, (uint32_t)value, (const uint8_t *)bytes, (size_t)length);
    (*env)->ReleaseByteArrayElements(env, receipt, bytes, JNI_ABORT);
    return result;
}
JNIEXPORT jint JNICALL Java_org_catsystem2_player_AndroidTweetHost_nativeCancel(
    JNIEnv *env, jclass type, jlong handle
) {
    (void)env; UNUSED_TYPES;
    return cs2_android_tweet_cancel((uint64_t)handle);
}
JNIEXPORT jint JNICALL Java_org_catsystem2_player_AndroidTweetHost_nativeCount(
    JNIEnv *env, jclass type, jbyteArray text, jbyteArray suffix
) {
    UNUSED_TYPES;
    if (text == NULL || suffix == NULL) return -1;
    jsize text_length = (*env)->GetArrayLength(env, text), suffix_length = (*env)->GetArrayLength(env, suffix);
    if (text_length < 0 || text_length > 4092 || suffix_length < 0 || suffix_length > 4092) return -1;
    uint8_t body[4093], tags[4093];
    (*env)->GetByteArrayRegion(env, text, 0, text_length, (jbyte *)body);
    (*env)->GetByteArrayRegion(env, suffix, 0, suffix_length, (jbyte *)tags);
    if ((*env)->ExceptionCheck(env)) return -1;
    return cs2_android_tweet_count(body, (size_t)text_length, tags, (size_t)suffix_length);
}
JNIEXPORT jbyteArray JNICALL JNI(nativeDialog)(JNIEnv *env, jclass type, jlong handle) {
    UNUSED_TYPES;
    return host_payload(env, handle, cs2_android_dialog, "Dialog request failed");
}
JNIEXPORT jint JNICALL JNI(nativeDialogResult)(
    JNIEnv *env, jclass type, jlong handle, jlong request, jint shown
) {
    (void)env; UNUSED_TYPES;
    return cs2_android_dialog_result((uint64_t)handle, (uint64_t)request, (int32_t)shown);
}
JNIEXPORT jint JNICALL JNI(nativeSystemOpenResult)(
    JNIEnv *env, jclass type, jlong handle, jlong request, jbyteArray failure
) {
    UNUSED_TYPES;
    if (failure == NULL) {
        return cs2_android_system_open_result((uint64_t)handle, (uint64_t)request, NULL, 0);
    }
    const jsize length = (*env)->GetArrayLength(env, failure);
    if (length < 0 || length > 32768) return -1;
    uint8_t *bytes = malloc((size_t)length + 1);
    if (bytes == NULL) return -1;
    (*env)->GetByteArrayRegion(env, failure, 0, length, (jbyte *)bytes);
    int result = -1;
    if (!(*env)->ExceptionCheck(env)) {
        result = cs2_android_system_open_result((uint64_t)handle, (uint64_t)request, bytes, (size_t)length);
    }
    free(bytes);
    return result;
}
static jint dispatch_input(
    jlong handle, jint kind, jint a, jint b, jlong event_to_ui_ms, jlong ui_to_worker_us
) {
    uint64_t input_mono_us = 0;
    if (perf.handle && perf.handle == (uint64_t)handle) {
        if (kind == 1 && a == 0) {
            input_mono_us = perf_now_ns() / 1000;
            __android_log_print(ANDROID_LOG_INFO, PERF_TAG, "primary_down session=%" PRIu64
                                " input_mono_us=%" PRIu64, perf.handle, input_mono_us);
        } else if (kind == 5 && a == 0) {
            perf_flush();
            perf.last_post_ns = 0;
        }
    }
    const jint result = cs2_android_input((uint64_t)handle, (uint32_t)kind, (int32_t)a, (int32_t)b);
    if (input_mono_us && event_to_ui_ms >= 0 && ui_to_worker_us >= 0) {
        __android_log_print(ANDROID_LOG_INFO, PERF_TAG, "primary_down_timing session=%" PRIu64
                            " input_mono_us=%" PRIu64 " event_to_ui_ms=%" PRId64 " ui_to_worker_us=%" PRId64,
                            (uint64_t)handle, input_mono_us, (int64_t)event_to_ui_ms, (int64_t)ui_to_worker_us);
    }
    return result;
}
JNIEXPORT jint JNICALL JNI(nativeInput)(
    JNIEnv *env, jclass type, jlong handle, jint kind, jint a, jint b
) {
    (void)env; UNUSED_TYPES;
    return dispatch_input(handle, kind, a, b, -1, -1);
}
JNIEXPORT jint JNICALL JNI(nativeTimedPrimaryDown)(
    JNIEnv *env, jclass type, jlong handle, jlong event_to_ui_ms, jlong ui_to_worker_us
) {
    (void)env; UNUSED_TYPES;
    return dispatch_input(handle, 1, 0, 0, event_to_ui_ms, ui_to_worker_us);
}
JNIEXPORT jint JNICALL JNI(nativeText)(
    JNIEnv *env, jclass type, jlong handle, jint kind, jbyteArray input, jint start, jint end
) {
    UNUSED_TYPES;
    if (input == NULL) return -1;
    const jsize length = (*env)->GetArrayLength(env, input);
    if (length < 0 || length > 1024 * 1024) return -1;
    uint8_t *bytes = malloc((size_t)length + 1);
    if (bytes == NULL) return -1;
    (*env)->GetByteArrayRegion(env, input, 0, length, (jbyte *)bytes);
    int result = -1;
    if (!(*env)->ExceptionCheck(env)) {
        result = cs2_android_text((uint64_t)handle, (uint32_t)kind, bytes, (size_t)length, start, end);
    }
    free(bytes);
    return result;
}
JNIEXPORT jbyteArray JNICALL JNI(nativeError)(JNIEnv *env, jclass type) {
    UNUSED_TYPES;
    const char *message = cs2_android_last_error();
    const size_t length = strlen(message);
    if (length > 2147483647u) return NULL;
    jbyteArray bytes = (*env)->NewByteArray(env, (jsize)length);
    if (bytes != NULL) (*env)->SetByteArrayRegion(env, bytes, 0, (jsize)length, (const jbyte *)message);
    return bytes;
}
JNIEXPORT jint JNICALL JNI(nativePresent)(JNIEnv *env, jclass type, jlong handle, jobject surface) {
    UNUSED_TYPES;
    const int timed = perf.handle && perf.handle == (uint64_t)handle;
    const bool measure = timed || hud_enabled;
    const uint64_t present_start = measure ? perf_now_ns() : 0;
    uint64_t lock_start = 0, copy_start = 0, post_start = 0, posted_at = 0;
    if (surface == NULL) return -2;
    ANativeWindow *window = ANativeWindow_fromSurface(env, surface);
    if (window == NULL) return -2;
    const int width = cs2_android_size((uint64_t)handle, 0);
    const int height = cs2_android_size((uint64_t)handle, 1);
    int result = -1;
    // The Android BufferQueue permits one producer. Disconnect EGL before
    // a Vulkan configure attempt and let Rust drop it before software fallback.
    if (cs2_android_vulkan_candidate(window)) {
        if (!cs2_egl_close()) {
            result = -2;
            goto finish;
        }
        if (measure) lock_start = copy_start = post_start = perf_now_ns();
        const int vulkan = cs2_android_present_vulkan((uint64_t)handle, window);
        if (vulkan <= 0) {
            if (vulkan == 0) {
                if (measure) posted_at = perf_now_ns();
                result = cs2_android_frame_posted((uint64_t)handle);
            } else result = vulkan;
            goto finish;
        }
    }
    enum Cs2EglResult backend = cs2_egl_prepare(window, width, height);
    if (backend == CS2_EGL_READY) {
        // EGL acquires a buffer inside swap, so the legacy lock column is zero.
        if (measure) lock_start = copy_start = perf_now_ns();
        result = cs2_android_copy_frame((uint64_t)handle, cs2_egl_pixels(), width, height, width);
        if (result == 0) {
            if (cs2_egl_draw()) {
                if (measure) post_start = perf_now_ns();
                if (cs2_egl_swap()) {
                    if (measure) posted_at = perf_now_ns();
                    result = cs2_android_frame_posted((uint64_t)handle);
                } else {
                    backend = cs2_egl_disable();
                }
            } else {
                backend = cs2_egl_disable();
            }
        }
    }
    if (backend == CS2_EGL_RETRY) result = -2;
    // An EGL frame that was copied but never swapped did not reach the window.
    if (backend == CS2_EGL_SOFTWARE) result = -1;
    if (backend == CS2_EGL_SOFTWARE && width > 0 && height > 0
        && ANativeWindow_setBuffersGeometry(window, width, height, WINDOW_FORMAT_RGBA_8888) == 0) {
        ANativeWindow_Buffer buffer;
        if (measure) lock_start = perf_now_ns();
        result = ANativeWindow_lock(window, &buffer, NULL);
        if (result == 0) {
            if (measure) copy_start = perf_now_ns();
            result = buffer.format == WINDOW_FORMAT_RGBA_8888
                ? cs2_android_copy_frame((uint64_t)handle, buffer.bits, buffer.width, buffer.height, buffer.stride)
                : -1;
            if (measure) post_start = perf_now_ns();
            const int posted = ANativeWindow_unlockAndPost(window);
            if (measure && posted == 0) posted_at = perf_now_ns();
            if (result == 0) result = posted == 0 ? cs2_android_frame_posted((uint64_t)handle) : posted;
        }
    }
finish:
    ANativeWindow_release(window);
    if (result == 0 && posted_at) {
        const uint64_t end = perf_now_ns();
        if (hud_enabled) {
            hud.last_submit_at_ns = posted_at;
            hud.present_ns = end - present_start;
            hud.copy_ns = post_start - copy_start;
            hud.swap_ns = posted_at - post_start;
        }
        if (timed) perf_record_post(present_start, lock_start, copy_start, post_start, posted_at, end);
    }
    return result;
}

JNIEXPORT jint JNICALL JNI(nativeDetachSurface)(JNIEnv *env, jclass type) {
    (void)env; UNUSED_TYPES;
    cs2_android_detach_vulkan();
    if (!cs2_egl_close()) return -2;
    hud.last_submit_at_ns = 0;
    return 0;
}

JNIEXPORT void JNICALL JNI(nativeSetHudEnabled)(JNIEnv *env, jclass type, jboolean enabled) {
    (void)env; UNUSED_TYPES;
    if (hud_enabled == (enabled != JNI_FALSE)) return;
    hud_enabled = enabled != JNI_FALSE;
    hud = (HudTimings){0};
    cs2_egl_set_stats_enabled(hud_enabled);
    cs2_android_vulkan_display_stats_enable(hud_enabled);
}

JNIEXPORT jint JNICALL JNI(nativeReadHud)(JNIEnv *env, jclass type, jlong handle, jlongArray output) {
    UNUSED_TYPES;
    if (!hud_enabled) return 0;
    uint64_t vulkan[7];
    const Cs2DisplayStats display = cs2_android_vulkan_display_stats(vulkan) == 1
        ? (Cs2DisplayStats){ (enum Cs2DisplaySource)vulkan[0], vulkan[1], vulkan[2], vulkan[3], vulkan[4], vulkan[5], vulkan[6] }
        : cs2_egl_display_stats();
    uint64_t sequence = 0, gpu_ns = 0;
    cs2_android_gpu_timing(&sequence, &gpu_ns);
    const jlong fields[] = {
        handle, (jlong)display.source, (jlong)display.generation, (jlong)display.displayed,
        (jlong)display.latest_ns, (jlong)display.queue_to_display_ns, (jlong)display.unresolved, (jlong)display.pending,
        (jlong)hud.last_submit_at_ns, (jlong)hud.present_ns, (jlong)hud.copy_ns, (jlong)hud.swap_ns,
        (jlong)hud.last_tick_ns, (jlong)hud.last_tick_at_ns, (jlong)sequence, (jlong)gpu_ns,
    };
    const jsize count = (jsize)(sizeof(fields) / sizeof(fields[0]));
    if (output == NULL || (*env)->GetArrayLength(env, output) < count) return -1;
    (*env)->SetLongArrayRegion(env, output, 0, count, fields);
    return (*env)->ExceptionCheck(env) ? -1 : 1;
}
