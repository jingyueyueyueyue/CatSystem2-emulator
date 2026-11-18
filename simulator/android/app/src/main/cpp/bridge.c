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

enum { PERF_SCRIPT_DETAIL = 5, PERF_PHASES = 13, PERF_BATCH = 8 };

typedef struct {
    uint64_t phase_ns[PERF_PHASES];
} HostFrameTiming;

extern uint64_t cs2_android_open(const char *, const char *);
extern int32_t cs2_android_close(uint64_t);
extern int32_t cs2_android_step(uint64_t, uint32_t);
extern int32_t cs2_android_step_timed(uint64_t, uint32_t, HostFrameTiming *);
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
extern const char *cs2_android_last_error(void);

#define JNI(name) Java_org_catsystem2_player_PlayerActivity_##name
#define UNUSED_TYPES (void)type
#define PERF_TAG "CS2Perf"

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

static void perf_record_post(uint64_t present_start, uint64_t lock_start, uint64_t copy_start,
                             uint64_t post_start, uint64_t posted_at, uint64_t present_end) {
    const PerfTick *tick = &perf.tick;
    const uint64_t interval = perf.last_post_ns ? (posted_at - perf.last_post_ns) / 1000 : 0;
    perf.last_post_ns = posted_at;
    perf.post_sequence++;
    if (perf.post_sequence == 1) {
        __android_log_print(ANDROID_LOG_INFO, PERF_TAG, "first_post session=%" PRIu64
                            " open_to_post_us=%" PRIu64, perf.handle,
                            (posted_at - perf.opened_from_ns) / 1000);
    }
    char row[640];
    const uint64_t *phase = tick->timing.phase_ns;
    uint64_t script = phase[0];
    for (size_t detail = PERF_SCRIPT_DETAIL; detail < PERF_PHASES; detail++) script += phase[detail];
    const int length = snprintf(row, sizeof(row), "f,%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64
        ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 "\n",
        perf.handle, perf.post_sequence, tick->frame, tick->step_calls, tick->step_ns / 1000,
        script / 1000, phase[1] / 1000, phase[2] / 1000, phase[3] / 1000, phase[4] / 1000,
        (present_end - present_start) / 1000, (copy_start - lock_start) / 1000,
        (post_start - copy_start) / 1000, (posted_at - post_start) / 1000, interval,
        posted_at / 1000, tick->first_step_ns ? (posted_at - tick->first_step_ns) / 1000 : 0,
        phase[5] / 1000, phase[6] / 1000, phase[7] / 1000, phase[8] / 1000,
        phase[9] / 1000, phase[10] / 1000, phase[11] / 1000, phase[12] / 1000);
    if (length > 0 && (size_t)length < sizeof(row)) {
        if (perf.log_size + (size_t)length >= sizeof(perf.log_batch)) perf_flush();
        memcpy(perf.log_batch + perf.log_size, row, (size_t)length + 1);
        perf.log_size += (size_t)length;
        perf.log_rows++;
        if (perf.post_sequence == 1 || perf.log_rows == PERF_BATCH) perf_flush();
    }
    perf.tick = (PerfTick){0};
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
    char *game_path = path_bytes(env, game);
    char *save_path = path_bytes(env, saves);
    uint64_t handle = 0;
    if (game_path != NULL && save_path != NULL) handle = cs2_android_open(game_path, save_path);
    free(game_path);
    free(save_path);
    if (start && handle) {
        perf = (PerfSession){ .handle = handle, .opened_from_ns = start };
        __android_log_print(ANDROID_LOG_INFO, PERF_TAG, "open session=%" PRIu64 " native_us=%" PRIu64,
                            handle, (perf_now_ns() - start) / 1000);
        __android_log_write(ANDROID_LOG_INFO, PERF_TAG,
            "f,session,post,frame,step_calls,step_us,script_us,image_read_us,image_decode_us,"
            "image_patch_us,compose_us,present_us,lock_us,copy_us,post_us,interval_us,post_mono_us,tick_to_post_us,"
            "script_input_us,script_root_us,script_children_us,script_fes_us,script_list_us,script_list_raster_us,"
            "script_motion_raster_us,script_button_us");
    }
    return (jlong)handle;
}

JNIEXPORT jint JNICALL JNI(nativeClose)(JNIEnv *env, jclass type, jlong handle) {
    (void)env; UNUSED_TYPES;
    if (perf.handle && perf.handle == (uint64_t)handle) {
        perf_flush();
        perf.handle = 0;
    }
    return cs2_android_close((uint64_t)handle);
}
JNIEXPORT jint JNICALL JNI(nativeStep)(JNIEnv *env, jclass type, jlong handle, jint budget) {
    (void)env; UNUSED_TYPES;
    if (!perf.handle || perf.handle != (uint64_t)handle) {
        return cs2_android_step((uint64_t)handle, (uint32_t)budget);
    }
    HostFrameTiming sample = {0};
    const uint64_t start = perf_now_ns();
    const int32_t result = cs2_android_step_timed((uint64_t)handle, (uint32_t)budget, &sample);
    perf.tick.step_ns += perf_now_ns() - start;
    if (perf.tick.step_calls++ == 0) perf.tick.first_step_ns = start;
    for (size_t phase = 0; phase < PERF_PHASES; phase++) {
        perf.tick.timing.phase_ns[phase] += sample.phase_ns[phase];
    }
    if (result == 1) perf.tick.frame = ++perf.frame_sequence;
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
JNIEXPORT jint JNICALL JNI(nativeInput)(
    JNIEnv *env, jclass type, jlong handle, jint kind, jint a, jint b
) {
    (void)env; UNUSED_TYPES;
    if (perf.handle && perf.handle == (uint64_t)handle) {
        if (kind == 1 && a == 0) {
            __android_log_print(ANDROID_LOG_INFO, PERF_TAG, "primary_down session=%" PRIu64
                                " input_mono_us=%" PRIu64, perf.handle, perf_now_ns() / 1000);
        } else if (kind == 5 && a == 0) {
            perf_flush();
            perf.last_post_ns = 0;
        }
    }
    return cs2_android_input((uint64_t)handle, (uint32_t)kind, (int32_t)a, (int32_t)b);
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
    const uint64_t present_start = timed ? perf_now_ns() : 0;
    uint64_t lock_start = 0, copy_start = 0, post_start = 0, posted_at = 0;
    if (surface == NULL) return -2;
    ANativeWindow *window = ANativeWindow_fromSurface(env, surface);
    if (window == NULL) return -2;
    const int width = cs2_android_size((uint64_t)handle, 0);
    const int height = cs2_android_size((uint64_t)handle, 1);
    int result = -1;
    if (width > 0 && height > 0
        && ANativeWindow_setBuffersGeometry(window, width, height, WINDOW_FORMAT_RGBA_8888) == 0) {
        ANativeWindow_Buffer buffer;
        if (timed) lock_start = perf_now_ns();
        result = ANativeWindow_lock(window, &buffer, NULL);
        if (result == 0) {
            if (timed) copy_start = perf_now_ns();
            result = cs2_android_copy_frame((uint64_t)handle, buffer.bits, buffer.width, buffer.height, buffer.stride);
            if (timed) post_start = perf_now_ns();
            const int posted = ANativeWindow_unlockAndPost(window);
            if (timed && posted == 0) posted_at = perf_now_ns();
            if (result == 0) result = posted;
        }
    }
    ANativeWindow_release(window);
    if (timed && result == 0 && posted_at) {
        perf_record_post(present_start, lock_start, copy_start, post_start, posted_at, perf_now_ns());
    }
    return result;
}
