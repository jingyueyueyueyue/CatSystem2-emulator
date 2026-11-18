/* MIT: independent adapter to the public Open JTalk1.11 / HTS1.10 APIs.
 * Source/attribution manifests are emitted by prepare_speech_backend.py. */
#ifndef CS2_SPEECH_H
#define CS2_SPEECH_H
#include <stddef.h>
#include <stdint.h>
#ifdef _WIN32
#define CS2_SPEECH_API __declspec(dllexport)
#else
#define CS2_SPEECH_API __attribute__((visibility("default")))
#endif
#ifdef __cplusplus
extern "C" {
#endif
typedef struct Cs2SpeechPcm {
    uint32_t sample_rate;
    uint32_t channels;
    size_t sample_count;
    int16_t *samples;
} Cs2SpeechPcm;
CS2_SPEECH_API uint32_t cs2_speech_abi_version(void);
CS2_SPEECH_API uint32_t cs2_speech_pcm_size(void);
CS2_SPEECH_API void *cs2_speech_open(const char *dictionary, const char *voice,
                                   char *diagnostic, size_t diagnostic_capacity);
CS2_SPEECH_API int cs2_speech_set_parameters(void *instance, double speed, double tone,
                                          char *diagnostic, size_t diagnostic_capacity);
CS2_SPEECH_API int cs2_speech_synthesize(void *instance, const char *utf8, size_t length,
                                       Cs2SpeechPcm *pcm, char *diagnostic,
                                       size_t diagnostic_capacity);
CS2_SPEECH_API void cs2_speech_free_pcm(Cs2SpeechPcm *pcm);
CS2_SPEECH_API void cs2_speech_close(void *instance);
#ifdef __cplusplus
}
#endif
#endif
