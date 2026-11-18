/* MIT. Written independently from public API contracts; no engine/DLL code.
 * Dependencies: Open JTalk1.11, HTS engine API1.10 (modified BSD), MeCab(BSD).
 * These libraries and their attribution files remain separate build inputs. */
#include "cs2_speech.h"
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#include <windows.h>
#endif
#include "HTS_engine.h"
#include "mecab.h"
#include "njd.h"
#include "jpcommon.h"
#include "text2mecab.h"
#include "mecab2njd.h"
#include "njd_set_pronunciation.h"
#include "njd_set_digit.h"
#include "njd_set_accent_phrase.h"
#include "njd_set_accent_type.h"
#include "njd_set_unvoiced_vowel.h"
#include "njd_set_long_vowel.h"
#include "njd2jpcommon.h"

#ifdef _WIN32
/* The MinGW recipe force-includes our name mapping in HTS. Dictionary paths
 * use MeCab's exact ACP path separately; the voice path stays UTF-8 here. */
FILE *cs2_speech_utf8_fopen(const char *path, const char *mode) {
    wchar_t wide_path[32768], wide_mode[32];
    if (!MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, path, -1, wide_path, 32768)
        || !MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, mode, -1, wide_mode, 32))
        return NULL;
    return _wfopen(wide_path, wide_mode);
}
#endif

typedef struct SpeechEngine {
    Mecab analyzer;
    NJD pronunciation;
    JPCommon labels;
    HTS_Engine acoustic;
} SpeechEngine;

static int fail(char *output, size_t capacity, const char *message) {
    if (output != NULL && capacity != 0) {
        snprintf(output, capacity, "%s", message);
        output[capacity - 1] = 0;
    }
    return -1;
}

static void refresh(SpeechEngine *engine) {
    HTS_Engine_refresh(&engine->acoustic);
    JPCommon_refresh(&engine->labels);
    NJD_refresh(&engine->pronunciation);
    Mecab_refresh(&engine->analyzer);
}

uint32_t cs2_speech_abi_version(void) { return 2; }
uint32_t cs2_speech_pcm_size(void) { return (uint32_t)sizeof(Cs2SpeechPcm); }

void cs2_speech_close(void *instance) {
    SpeechEngine *engine = instance;
    if (engine == NULL) return;
    HTS_Engine_clear(&engine->acoustic);
    JPCommon_clear(&engine->labels);
    NJD_clear(&engine->pronunciation);
    Mecab_clear(&engine->analyzer);
    free(engine);
}

void *cs2_speech_open(const char *dictionary, const char *voice,
                      char *diagnostic, size_t capacity) {
    if (dictionary == NULL || voice == NULL || !*dictionary || !*voice) {
        fail(diagnostic, capacity, "speech dictionary and HTS voice paths are required");
        return NULL;
    }
    SpeechEngine *engine = calloc(1, sizeof(*engine));
    if (engine == NULL) {
        fail(diagnostic, capacity, "speech engine allocation failed");
        return NULL;
    }
    Mecab_initialize(&engine->analyzer);
    NJD_initialize(&engine->pronunciation);
    JPCommon_initialize(&engine->labels);
    HTS_Engine_initialize(&engine->acoustic);
    /* PCM is returned to the existing audio host. Never open a second device. */
    HTS_Engine_set_audio_buff_size(&engine->acoustic, 0);
    char *voices[1] = { (char *)voice };
    if (!Mecab_load(&engine->analyzer, dictionary)) {
        fail(diagnostic, capacity, "Open JTalk could not load the UTF-8 dictionary");
        cs2_speech_close(engine);
        return NULL;
    }
    if (!HTS_Engine_load(&engine->acoustic, voices, 1)) {
        fail(diagnostic, capacity, "HTS could not load the supplied voice model");
        cs2_speech_close(engine);
        return NULL;
    }
    HTS_Engine_set_audio_buff_size(&engine->acoustic, 0);
    const char *format = HTS_Engine_get_fullcontext_label_format(&engine->acoustic);
    if (format == NULL || strcmp(format, "HTS_TTS_JPN") != 0) {
        fail(diagnostic, capacity, "HTS voice does not use Open JTalk Japanese context labels");
        cs2_speech_close(engine);
        return NULL;
    }
    size_t rate = HTS_Engine_get_sampling_frequency(&engine->acoustic);
    if (rate == 0 || rate > 192000) {
        fail(diagnostic, capacity, "HTS voice sampling frequency is unsupported");
        cs2_speech_close(engine);
        return NULL;
    }
    return engine;
}

void cs2_speech_free_pcm(Cs2SpeechPcm *pcm) {
    if (pcm == NULL) return;
    free(pcm->samples);
    memset(pcm, 0, sizeof(*pcm));
}

int cs2_speech_set_parameters(void *instance, double speed, double tone,
                              char *diagnostic, size_t capacity) {
    SpeechEngine *engine = instance;
    if (engine == NULL || !isfinite(speed) || speed <= 0.0 || !isfinite(tone))
        return fail(diagnostic, capacity, "speech requires positive speed and finite speed/tone");
    HTS_Engine_set_speed(&engine->acoustic, speed);
    /* Public HTS API replaces additional_half_tone; it does not accumulate
     * successive calls. Loaded PCM is immutable and never retuned here. */
    HTS_Engine_add_half_tone(&engine->acoustic, tone);
    return 0;
}

int cs2_speech_synthesize(void *instance, const char *utf8, size_t length,
                          Cs2SpeechPcm *pcm, char *diagnostic, size_t capacity) {
    SpeechEngine *engine = instance;
    if (pcm != NULL) memset(pcm, 0, sizeof(*pcm));
    if (engine == NULL || pcm == NULL || utf8 == NULL || length == 0 || length > 4096
        || memchr(utf8, 0, length) != NULL || utf8[length] != 0)
        return fail(diagnostic, capacity, "speech requires1..4096 UTF-8 bytes");
    char normalized[4096 * 8 + 1024];
    refresh(engine);
    text2mecab(normalized, utf8);
    if (!Mecab_analysis(&engine->analyzer, normalized)) {
        refresh(engine);
        return fail(diagnostic, capacity, "Open JTalk text analysis failed");
    }
    mecab2njd(&engine->pronunciation, Mecab_get_feature(&engine->analyzer),
             Mecab_get_size(&engine->analyzer));
    njd_set_pronunciation(&engine->pronunciation);
    njd_set_digit(&engine->pronunciation);
    njd_set_accent_phrase(&engine->pronunciation);
    njd_set_accent_type(&engine->pronunciation);
    njd_set_unvoiced_vowel(&engine->pronunciation);
    njd_set_long_vowel(&engine->pronunciation);
    njd2jpcommon(&engine->labels, &engine->pronunciation);
    JPCommon_make_label(&engine->labels);
    int label_count = JPCommon_get_label_size(&engine->labels);
    if (label_count <= 2 || label_count > 4096
        || !HTS_Engine_generate_state_sequence_from_strings(
            &engine->acoustic, JPCommon_get_label_feature(&engine->labels), (size_t)label_count)) {
        refresh(engine);
        return fail(diagnostic, capacity, "speech produced no bounded acoustic label sequence");
    }
    size_t rate = HTS_Engine_get_sampling_frequency(&engine->acoustic);
    size_t period = HTS_Engine_get_fperiod(&engine->acoustic);
    size_t frames = HTS_Engine_get_total_frame(&engine->acoustic);
    if (period == 0 || frames == 0 || frames > (rate * 60) / period
        || !HTS_Engine_generate_parameter_sequence(&engine->acoustic)
        || !HTS_Engine_generate_sample_sequence(&engine->acoustic)) {
        refresh(engine);
        return fail(diagnostic, capacity, "speech synthesis failed or exceeded60 seconds");
    }
    size_t count = HTS_Engine_get_nsamples(&engine->acoustic);
    if (count == 0 || count > rate * 60 || count > (64u * 1024u * 1024u) / sizeof(int16_t)) {
        refresh(engine);
        return fail(diagnostic, capacity, "speech PCM exceeds its bounded allocation");
    }
    pcm->samples = malloc(count * sizeof(int16_t));
    if (pcm->samples == NULL) {
        refresh(engine);
        return fail(diagnostic, capacity, "speech PCM allocation failed");
    }
    for (size_t index = 0; index < count; ++index) {
        double value = HTS_Engine_get_generated_speech(&engine->acoustic, index);
        if (!isfinite(value)) {
            cs2_speech_free_pcm(pcm);
            refresh(engine);
            return fail(diagnostic, capacity, "speech model generated a nonfinite sample");
        }
        value = fmax(-32768.0, fmin(32767.0, value));
        pcm->samples[index] = (int16_t)lrint(value);
    }
    pcm->sample_rate = (uint32_t)rate;
    pcm->channels = 1;
    pcm->sample_count = count;
    refresh(engine);
    return 0;
}
