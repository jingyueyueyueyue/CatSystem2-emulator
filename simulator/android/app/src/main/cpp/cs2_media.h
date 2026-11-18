/* SPDX-License-Identifier: MIT
 * Independent FFmpeg public-C-API bridge. FFmpeg itself is separately licensed
 * LGPL-2.1-or-later in the pinned non-GPL shared-library configuration.
 * Source/version/configuration are recorded by prepare_android_ffmpeg.py.
 */
#ifndef CS2_MEDIA_H
#define CS2_MEDIA_H
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define CS2_MEDIA_ABI 1u
#define CS2_MEDIA_VIDEO 1
#define CS2_MEDIA_AUDIO 2
#define CS2_MEDIA_DISPLAY_TRANSFORMED 1u
#define CS2_MEDIA_COLOR_SRGB 2u
#define CS2_MEDIA_AUDIO_RECONFIGURED 4u
#define CS2_MEDIA_VIDEO_DURATION 8u
#define CS2_MEDIA_SEEK_SIZE 0x10000
#define CS2_MEDIA_SEEK_FORCE 0x20000
#if defined(__GNUC__)
#define CS2_MEDIA_API __attribute__((visibility("default")))
#else
#define CS2_MEDIA_API
#endif

typedef struct CS2Media CS2Media;
typedef struct CS2MediaIO {
    void *opaque;
    /* Positive bytes, zero validated EOF, -1 error, -2 cancellation.
     * Implementations may return a short read. The supplied bytes are copied
     * into FFmpeg-owned storage; original game/host addresses are never used.
     */
    int32_t (*read)(void *opaque, uint8_t *bytes, int32_t capacity);
    /* SEEK_SET/CUR/END or SEEK_SIZE. Return new position/size, -1 error,
     * -2 cancellation, -3 unknown size/unsupported seek.
     */
    int64_t (*seek)(void *opaque, int64_t offset, int32_t whence);
    int32_t (*cancelled)(void *opaque);
} CS2MediaIO;

typedef struct CS2MediaInfo {
    uint32_t abi;
    uint32_t width;
    uint32_t height;
    uint32_t has_audio;
    uint32_t input_audio_rate;
    uint32_t input_audio_channels;
    uint32_t fps_numerator;
    uint32_t fps_denominator;
    int64_t duration_us; /* -1 when not established by stream/container data */
} CS2MediaInfo;

typedef struct CS2MediaFrame {
    int32_t kind;
    uint32_t flags; /* DISPLAY_TRANSFORMED/COLOR_SRGB or AUDIO_RECONFIGURED */
    double pts_seconds;
    uint32_t width;
    uint32_t height;
    uint32_t sample_frames;
    uint32_t sample_rate;
    uint32_t channels;
    uint32_t reserved; /* VIDEO_DURATION: real display duration in microseconds */
    uint64_t byte_length;
    /* Owned by decoder until next cs2_media_read/close. Video is tightly
     * packed SDR sRGB RGBA8, with stream/frame affine display transform
     * already applied. Audio stays native-endian PCM16, stereo 48000Hz even
     * if input sample format/rate/layout changes. Old resampler tail is
     * emitted before the first event using the new input format.
     */
    const uint8_t *data;
} CS2MediaFrame;

CS2_MEDIA_API uint32_t cs2_media_abi_version(void);
CS2_MEDIA_API uint32_t cs2_media_info_size(void);
CS2_MEDIA_API uint32_t cs2_media_frame_size(void);
CS2_MEDIA_API const char *cs2_media_build_version(void);
CS2_MEDIA_API CS2Media *cs2_media_open(const CS2MediaIO *io, const char *demuxer, uint32_t decode_mask,
                       CS2MediaInfo *info, char *error, size_t error_capacity);
/* decode_mask bit0 selects video, bit1 audio;3 opens both for metadata probing.
 * Independent video/audio workers avoid a multiplexed queue deadlocking while
 * one output waits for the other's clock. Both use the same source identity.
 */
/* 1 = one actual decoded frame, 0 = validated decoder/resampler EOF,
 * negative = failure. Does not allocate/store the entire encoded movie.
 */
CS2_MEDIA_API int32_t cs2_media_read(CS2Media *media, CS2MediaFrame *frame);
CS2_MEDIA_API const char *cs2_media_last_error(const CS2Media *media);
CS2_MEDIA_API void cs2_media_close(CS2Media *media);

#ifdef __cplusplus
}
#endif
#endif
