/* MIT: force-included only when building the independent HTS static library.
 * A compile-time name mapping also covers MinGW dllimport fopen references,
 * which a linker's --wrap=fopen alone cannot reliably replace. */
#ifndef CS2_SPEECH_UTF8_IO_H
#define CS2_SPEECH_UTF8_IO_H
#ifdef _WIN32
#include <stdio.h>
#ifdef __cplusplus
extern "C" {
#endif
FILE *cs2_speech_utf8_fopen(const char *path, const char *mode);
#ifdef __cplusplus
}
#endif
#define fopen cs2_speech_utf8_fopen
#endif
#endif
