/* SPDX-License-Identifier: MIT
 * Independent adapter written against FFmpeg8.0.2 public headers/APIs.
 * No FFmpeg implementation, reference-engine code or examples are copied.
 */
#include "cs2_media.h"
#include <errno.h>
#include <limits.h>
#include <math.h>
#include <stdio.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>
#include <libavutil/channel_layout.h>
#include <libavutil/error.h>
#include <libavutil/imgutils.h>
#include <libavutil/mem.h>
#include <libswresample/swresample.h>
#include <libswscale/swscale.h>

#if LIBAVCODEC_VERSION_MAJOR != 62 || LIBAVFORMAT_VERSION_MAJOR != 62
#error "cs2_media requires the pinned FFmpeg8.x ABI; do not substitute system headers"
#endif

#define IO_BYTES (64 * 1024)
#define MAX_PIXELS (16u * 1024u * 1024u)
#define MAX_AUDIO_FRAMES (256 * 1024)
#define OUTPUT_RATE 48000
#define OUTPUT_CHANNELS 2

struct CS2Media {
    CS2MediaIO io;
    AVFormatContext *format;
    AVIOContext *avio;
    AVCodecContext *codec[2];
    int stream[2];
    AVPacket *packet;
    AVFrame *frame;
    AVFrame *converted;
    struct SwsContext *scale;
    struct SwrContext *resample;
    AVChannelLayout audio_layout;
    int audio_rate;
    int audio_format;
    uint8_t *rgba;
    size_t rgba_size;
    uint8_t *pcm;
    size_t pcm_size;
    int pending_packet;
    int receiving;
    int demux_ended;
    int flush_sent[2];
    int drained[2];
    int resample_drained;
    int audio_frame_pending;
    int audio_reconfigured;
    int audio_output_seen;
    int packet_needs_receive;
    uint32_t decode_mask;
    double origin;
    double next_video;
    double next_audio;
    double frame_period;
    char diagnostic[1024];
};

static int interrupted(void *opaque) {
    CS2Media *media = opaque;
    return media->io.cancelled && media->io.cancelled(media->io.opaque);
}

static int failure(CS2Media *media, int status, const char *operation) {
    char detail[AV_ERROR_MAX_STRING_SIZE];
    if (status >= 0) status = AVERROR_INVALIDDATA;
    if (av_strerror(status, detail, sizeof(detail)) < 0)
        snprintf(detail, sizeof(detail), "status %d", status);
    snprintf(media->diagnostic, sizeof(media->diagnostic), "%s: %s (%d)",
             operation, detail, status);
    return status;
}

static int read_input(void *opaque, uint8_t *bytes, int capacity) {
    CS2Media *media = opaque;
    if (interrupted(media)) return AVERROR_EXIT;
    int result = media->io.read(media->io.opaque, bytes, capacity);
    if (result == -2) return AVERROR_EXIT;
    if (result < 0 || result > capacity) return AVERROR(EIO);
    return result ? result : AVERROR_EOF;
}

static int64_t seek_input(void *opaque, int64_t offset, int whence) {
    CS2Media *media = opaque;
    if (interrupted(media)) return AVERROR_EXIT;
    if (!media->io.seek) return AVERROR(ENOSYS);
    int64_t result = media->io.seek(media->io.opaque, offset, whence);
    if (result == -2) return AVERROR_EXIT;
    if (result == -3) return AVERROR(ENOSYS);
    return result < 0 ? AVERROR(EIO) : result;
}

static int decoder_open(CS2Media *media, int slot) {
    AVStream *stream = media->format->streams[media->stream[slot]];
    const AVCodec *decoder = avcodec_find_decoder(stream->codecpar->codec_id);
    if (!decoder) return failure(media, AVERROR_DECODER_NOT_FOUND, "codec is absent from Android shared libraries");
    media->codec[slot] = avcodec_alloc_context3(decoder);
    if (!media->codec[slot]) return failure(media, AVERROR(ENOMEM), "allocate codec");
    int status = avcodec_parameters_to_context(media->codec[slot], stream->codecpar);
    if (status < 0) return failure(media, status, "copy codec parameters");
    media->codec[slot]->pkt_timebase = stream->time_base;
    media->codec[slot]->thread_count = 2;
    media->codec[slot]->max_pixels = MAX_PIXELS;
    status = avcodec_open2(media->codec[slot], decoder, NULL);
    return status < 0 ? failure(media, status, "open codec") : 0;
}

static int select_streams(CS2Media *media) {
    /* Ask for the intended stream independently of decoder availability.
     * An unavailable default codec must produce an explicit decoder error,
     * not silently substitute another language/track which happens to decode.
     */
    int video = av_find_best_stream(media->format, AVMEDIA_TYPE_VIDEO, -1, -1, NULL, 0);
    if (video >= 0 && (media->format->streams[video]->disposition & AV_DISPOSITION_ATTACHED_PIC))
        video = -1;
    if (video < 0) {
        for (unsigned i = 0; i < media->format->nb_streams; ++i) {
            const AVStream *stream = media->format->streams[i];
            if (stream->codecpar->codec_type != AVMEDIA_TYPE_VIDEO ||
                (stream->disposition & AV_DISPOSITION_ATTACHED_PIC)) continue;
            if (video < 0 || (stream->disposition & AV_DISPOSITION_DEFAULT))
                video = (int)i;
            if (stream->disposition & AV_DISPOSITION_DEFAULT) break;
        }
    }
    if (video < 0)
        return failure(media, AVERROR_STREAM_NOT_FOUND, "media contains no moving-picture video");
    int audio = av_find_best_stream(media->format, AVMEDIA_TYPE_AUDIO, -1, video, NULL, 0);
    if (audio < 0 && audio != AVERROR_STREAM_NOT_FOUND)
        return failure(media, audio, "select movie audio track");
    media->stream[0] = video;
    media->stream[1] = audio < 0 ? -1 : audio;
    for (unsigned i = 0; i < media->format->nb_streams; ++i) {
        if ((int)i != media->stream[0] && (int)i != media->stream[1])
            media->format->streams[i]->discard = AVDISCARD_ALL;
    }
    return 0;
}

void cs2_media_close(CS2Media *media) {
    if (!media) return;
    av_packet_free(&media->packet);
    av_frame_free(&media->frame);
    av_frame_free(&media->converted);
    avcodec_free_context(&media->codec[0]);
    avcodec_free_context(&media->codec[1]);
    sws_freeContext(media->scale);
    swr_free(&media->resample);
    av_channel_layout_uninit(&media->audio_layout);
    av_free(media->rgba);
    av_free(media->pcm);
    avformat_close_input(&media->format);
    if (media->avio) {
        /* AVIO may replace its initially supplied buffer. Free the current
         * public buffer pointer, then the context, exactly once. */
        av_freep(&media->avio->buffer);
        avio_context_free(&media->avio);
    }
    av_free(media);
}

uint32_t cs2_media_abi_version(void) { return CS2_MEDIA_ABI; }
uint32_t cs2_media_info_size(void) { return (uint32_t)sizeof(CS2MediaInfo); }
uint32_t cs2_media_frame_size(void) { return (uint32_t)sizeof(CS2MediaFrame); }
const char *cs2_media_build_version(void) { return av_version_info(); }
const char *cs2_media_last_error(const CS2Media *media) {
    return media ? media->diagnostic : "Android media decoder is absent";
}

typedef struct VideoTransform {
    double a, b, c, d, minimum_x, minimum_y, determinant;
    int width, height, identity;
} VideoTransform;

/* Public display-matrix convention is documented in libavutil/display.h:
 * x'=a*x+c*y+tx, y'=b*x+d*y+ty. Normalize the destination bounding box so
 * container translation does not turn a 90-degree rotation into clipping.
 * Frame side data overrides stream codec side data; never run a game DLL.
 */
static int video_transform(CS2Media *media, const AVFrame *frame,
                           int width, int height, VideoTransform *transform) {
    const uint8_t *data = NULL;
    size_t size = 0;
    if (frame) {
        const AVFrameSideData *side = av_frame_get_side_data(frame, AV_FRAME_DATA_DISPLAYMATRIX);
        if (side) { data = side->data; size = side->size; }
    }
    if (!data) {
        const AVCodecParameters *parameters =
            media->format->streams[media->stream[0]]->codecpar;
        const AVPacketSideData *side = av_packet_side_data_get(
            parameters->coded_side_data, parameters->nb_coded_side_data,
            AV_PKT_DATA_DISPLAYMATRIX);
        if (side) { data = side->data; size = side->size; }
    }
    memset(transform, 0, sizeof(*transform));
    transform->a = transform->d = 1.0;
    if (data) {
        int32_t matrix[9];
        if (size < sizeof(matrix))
            return failure(media, AVERROR_INVALIDDATA, "truncated video display matrix");
        memcpy(matrix, data, sizeof(matrix));
        if (matrix[2] || matrix[5] || !matrix[8])
            return failure(media, AVERROR_PATCHWELCOME, "perspective/singular video display matrix");
        double divisor = matrix[8] / 1073741824.0;
        transform->a = matrix[0] / 65536.0 / divisor;
        transform->b = matrix[1] / 65536.0 / divisor;
        transform->c = matrix[3] / 65536.0 / divisor;
        transform->d = matrix[4] / 65536.0 / divisor;
    }
    /* Matroska display dimensions and MPEG anamorphic pixels are represented
     * by SAR. Scale the source horizontal coordinate BEFORE display rotation.
     * The public helper resolves frame/stream/container precedence.
     */
    AVRational aspect = av_guess_sample_aspect_ratio(media->format,
        media->format->streams[media->stream[0]], (AVFrame *)frame);
    if (aspect.num > 0 && aspect.den > 0) {
        double ratio = av_q2d(aspect);
        transform->a *= ratio;
        transform->b *= ratio;
    }
    transform->determinant = transform->a * transform->d - transform->b * transform->c;
    if (!isfinite(transform->determinant) || fabs(transform->determinant) < 1e-12)
        return failure(media, AVERROR_INVALIDDATA, "invalid video display transform");
    double x[4] = {0.0, transform->a * width, transform->c * height,
                   transform->a * width + transform->c * height};
    double y[4] = {0.0, transform->b * width, transform->d * height,
                   transform->b * width + transform->d * height};
    double maximum_x = x[0], maximum_y = y[0];
    for (int i = 0; i < 4; ++i) {
        transform->minimum_x = fmin(transform->minimum_x, x[i]);
        transform->minimum_y = fmin(transform->minimum_y, y[i]);
        maximum_x = fmax(maximum_x, x[i]);
        maximum_y = fmax(maximum_y, y[i]);
    }
    double output_width = ceil(maximum_x - transform->minimum_x - 1e-9);
    double output_height = ceil(maximum_y - transform->minimum_y - 1e-9);
    if (!isfinite(output_width) || !isfinite(output_height) ||
        output_width < 1 || output_height < 1 || output_width > 65535 ||
        output_height > 65535 || output_width * output_height > MAX_PIXELS)
        return failure(media, AVERROR_INVALIDDATA, "display-transformed video exceeds limit");
    transform->width = (int)output_width;
    transform->height = (int)output_height;
    transform->identity = transform->a == 1.0 && transform->b == 0.0 &&
                          transform->c == 0.0 && transform->d == 1.0;
    return 0;
}

CS2Media *cs2_media_open(const CS2MediaIO *io, const char *demuxer, uint32_t decode_mask,
                       CS2MediaInfo *info, char *error, size_t capacity) {
    CS2Media *media = NULL;
    if (error && capacity) error[0] = 0;
    if (!io || !io->read || !info || !demuxer || !decode_mask || (decode_mask & ~3u)) {
        if (error && capacity) snprintf(error, capacity, "invalid Android media open arguments");
        return NULL;
    }
    media = av_mallocz(sizeof(*media));
    if (!media) {
        if (error && capacity) snprintf(error, capacity, "allocate Android media decoder");
        return NULL;
    }
    media->io = *io;
    media->decode_mask = decode_mask;
    media->stream[0] = media->stream[1] = -1;
    media->receiving = -1;
    media->format = avformat_alloc_context();
    uint8_t *buffer = av_malloc(IO_BYTES);
    if (!media->format || !buffer) {
        av_free(buffer);
        failure(media, AVERROR(ENOMEM), "allocate demuxer/AVIO buffer");
        goto failed;
    }
    media->avio = avio_alloc_context(buffer, IO_BYTES, 0, media, read_input, NULL, seek_input);
    if (!media->avio) {
        av_free(buffer);
        failure(media, AVERROR(ENOMEM), "allocate AVIO context");
        goto failed;
    }
    media->format->pb = media->avio;
    media->format->flags |= AVFMT_FLAG_CUSTOM_IO;
    media->format->interrupt_callback.callback = interrupted;
    media->format->interrupt_callback.opaque = media;
    media->format->probesize = 1024 * 1024;
    media->format->max_analyze_duration = 5 * AV_TIME_BASE;
    const AVInputFormat *input = av_find_input_format(demuxer);
    if (!input) {
        failure(media, AVERROR_DEMUXER_NOT_FOUND, "demuxer is absent from Android shared libraries");
        goto failed;
    }
    int status = avformat_open_input(&media->format, NULL, input, NULL);
    if (status < 0) { failure(media, status, "open media through archive AVIO"); goto failed; }
    status = avformat_find_stream_info(media->format, NULL);
    if (status < 0) { failure(media, status, "probe media streams"); goto failed; }
    if (select_streams(media) < 0) goto failed;
    for (int slot = 0; slot < 2; ++slot) {
        if (media->stream[slot] < 0 || !(decode_mask & (1u << slot))) {
            media->drained[slot] = media->flush_sent[slot] = 1;
        } else if (decoder_open(media, slot) < 0) goto failed;
    }
    media->packet = av_packet_alloc();
    media->frame = av_frame_alloc();
    media->converted = av_frame_alloc();
    if (!media->packet || !media->frame || !media->converted) {
        failure(media, AVERROR(ENOMEM), "allocate packet/frame");
        goto failed;
    }
    AVStream *video = media->format->streams[media->stream[0]];
    int width = video->codecpar->width, height = video->codecpar->height;
    if (width <= 0 || height <= 0 || (uint64_t)width * (uint64_t)height > MAX_PIXELS) {
        failure(media, AVERROR_INVALIDDATA, "invalid/oversized video dimensions");
        goto failed;
    }
    memset(info, 0, sizeof(*info));
    info->abi = CS2_MEDIA_ABI;
    VideoTransform transform;
    if (video_transform(media, NULL, width, height, &transform) < 0) goto failed;
    info->width = (uint32_t)transform.width;
    info->height = (uint32_t)transform.height;
    info->has_audio = media->stream[1] >= 0;
    if (info->has_audio) {
        AVCodecParameters *audio = media->format->streams[media->stream[1]]->codecpar;
        info->input_audio_rate = (uint32_t)audio->sample_rate;
        info->input_audio_channels = (uint32_t)audio->ch_layout.nb_channels;
    }
    if (media->drained[1]) media->resample_drained = 1;
    AVRational fps = av_guess_frame_rate(media->format, video, NULL);
    if (fps.num > 0 && fps.den > 0) {
        info->fps_numerator = (uint32_t)fps.num;
        info->fps_denominator = (uint32_t)fps.den;
        media->frame_period = av_q2d(av_inv_q(fps));
    } else media->frame_period = 1.0 / 30.0;
    info->duration_us = video->duration != AV_NOPTS_VALUE && video->duration >= 0
        ? av_rescale_q(video->duration, video->time_base, AV_TIME_BASE_Q)
        : media->format->duration;
    int have_origin = media->format->start_time != AV_NOPTS_VALUE;
    media->origin = have_origin ? media->format->start_time / (double)AV_TIME_BASE : 0.0;
    for (int slot = 0; slot < 2; ++slot) {
        if (media->stream[slot] < 0) continue;
        AVStream *stream = media->format->streams[media->stream[slot]];
        if (stream->start_time != AV_NOPTS_VALUE) {
            double start = stream->start_time * av_q2d(stream->time_base);
            if (!have_origin || start < media->origin) media->origin = start;
            have_origin = 1;
        }
    }
    return media;
failed:
    if (error && capacity) snprintf(error, capacity, "%s", media->diagnostic);
    cs2_media_close(media);
    return NULL;
}

static double frame_time(CS2Media *media, int slot, double fallback) {
    int64_t pts = media->frame->best_effort_timestamp;
    if (pts == AV_NOPTS_VALUE) pts = media->frame->pts;
    if (pts == AV_NOPTS_VALUE) return fallback;
    AVStream *stream = media->format->streams[media->stream[slot]];
    return pts * av_q2d(stream->time_base) - media->origin;
}

static int video_frame(CS2Media *media, CS2MediaFrame *output) {
    AVFrame *frame = media->frame;
    if (frame->width <= 0 || frame->height <= 0 ||
        (uint64_t)frame->width * (uint64_t)frame->height > MAX_PIXELS)
        return failure(media, AVERROR_INVALIDDATA, "decoded video size exceeds limit");
    if (frame->crop_left >= (size_t)frame->width ||
        frame->crop_right >= (size_t)frame->width - frame->crop_left ||
        frame->crop_top >= (size_t)frame->height ||
        frame->crop_bottom >= (size_t)frame->height - frame->crop_top)
        return failure(media, AVERROR_INVALIDDATA, "decoded video crop exceeds frame");
    int display_width = frame->width - (int)frame->crop_left - (int)frame->crop_right;
    int display_height = frame->height - (int)frame->crop_top - (int)frame->crop_bottom;
    VideoTransform transform;
    if (video_transform(media, frame, display_width, display_height, &transform) < 0)
        return AVERROR_INVALIDDATA;
    size_t bytes = (size_t)transform.width * (size_t)transform.height * 4;
    if (bytes > media->rgba_size) {
        uint8_t *replacement = av_realloc(media->rgba, bytes);
        if (!replacement) return failure(media, AVERROR(ENOMEM), "allocate RGBA");
        media->rgba = replacement;
        media->rgba_size = bytes;
    }
    const AVCodecParameters *parameters = media->format->streams[media->stream[0]]->codecpar;
    if (frame->colorspace == AVCOL_SPC_UNSPECIFIED) frame->colorspace = parameters->color_space;
    if (frame->color_range == AVCOL_RANGE_UNSPECIFIED) frame->color_range = parameters->color_range;
    if (frame->color_primaries == AVCOL_PRI_UNSPECIFIED) frame->color_primaries = parameters->color_primaries;
    if (frame->color_trc == AVCOL_TRC_UNSPECIFIED) frame->color_trc = parameters->color_trc;
    if (frame->color_trc == AVCOL_TRC_SMPTE2084 || frame->color_trc == AVCOL_TRC_ARIB_STD_B67)
        return failure(media, AVERROR_PATCHWELCOME, "HDR movie requires an explicit tone-mapping policy");
    if (!media->scale) {
        media->scale = sws_alloc_context();
        if (!media->scale) return failure(media, AVERROR(ENOMEM), "allocate dynamic color converter");
        media->scale->threads = 2;
        media->scale->flags = SWS_BILINEAR;
    }
    av_frame_unref(media->converted);
    media->converted->format = AV_PIX_FMT_RGBA;
    media->converted->width = frame->width;
    media->converted->height = frame->height;
    media->converted->color_range = AVCOL_RANGE_JPEG;
    media->converted->colorspace = AVCOL_SPC_RGB;
    media->converted->color_primaries = AVCOL_PRI_BT709;
    media->converted->color_trc = AVCOL_TRC_IEC61966_2_1;
    /* The pinned FFmpeg8 dynamic API consumes per-frame range, matrix,
     * primaries and transfer metadata and recreates its graph when they
     * change. A legacy cached sws_scale context does not do this. */
    int status = sws_scale_frame(media->scale, media->converted, frame);
    if (status < 0) return failure(media, status, "convert video metadata to SDR sRGB RGBA");
    /* Crop only the converted packed RGBA. This represents odd crop origins
     * exactly without handing unaligned YUV planes to FFmpeg or silently
     * rounding away a codec's display crop. */
    const uint8_t *pixels = media->converted->data[0] +
        (ptrdiff_t)frame->crop_top * media->converted->linesize[0] + frame->crop_left * 4;
    for (int y = 0; y < transform.height; ++y) {
        if (!(y & 63) && interrupted(media))
            return failure(media, AVERROR_EXIT, "display transform cancelled");
        uint8_t *destination = media->rgba + (size_t)y * transform.width * 4;
        if (transform.identity) {
            memcpy(destination, pixels +
                   (ptrdiff_t)y * media->converted->linesize[0], (size_t)transform.width * 4);
            continue;
        }
        for (int x = 0; x < transform.width; ++x) {
            double dx = x + 0.5 + transform.minimum_x;
            double dy = y + 0.5 + transform.minimum_y;
            double sx = (transform.d * dx - transform.c * dy) / transform.determinant;
            double sy = (-transform.b * dx + transform.a * dy) / transform.determinant;
            uint8_t *pixel = destination + (size_t)x * 4;
            if (sx < 0 || sy < 0 || sx >= display_width || sy >= display_height) {
                memset(pixel, 0, 4);
                continue;
            }
            /* Pixel-center inverse mapping also handles flips and arbitrary
             * affine rotation. Quarter turns sample exactly on source centers. */
            double px = sx - 0.5, py = sy - 0.5;
            int left = (int)floor(px), top = (int)floor(py);
            double fx = px - left, fy = py - top;
            for (int component = 0; component < 4; ++component) {
                double value = 0.0;
                for (int row = 0; row < 2; ++row) {
                    int iy = top + row;
                    if (iy < 0) iy = 0;
                    if (iy >= display_height) iy = display_height - 1;
                    const uint8_t *source = pixels +
                        (ptrdiff_t)iy * media->converted->linesize[0];
                    for (int column = 0; column < 2; ++column) {
                        int ix = left + column;
                        if (ix < 0) ix = 0;
                        if (ix >= display_width) ix = display_width - 1;
                        value += source[(size_t)ix * 4 + component] *
                            (column ? fx : 1.0 - fx) * (row ? fy : 1.0 - fy);
                    }
                }
                pixel[component] = (uint8_t)fmin(255.0, fmax(0.0, floor(value + 0.5)));
            }
        }
    }
    output->kind = CS2_MEDIA_VIDEO;
    output->flags = CS2_MEDIA_COLOR_SRGB |
        (transform.identity ? 0u : CS2_MEDIA_DISPLAY_TRANSFORMED);
    output->width = (uint32_t)transform.width;
    output->height = (uint32_t)transform.height;
    output->byte_length = bytes;
    output->data = media->rgba;
    output->pts_seconds = frame_time(media, 0, media->next_video);
    if (!isfinite(output->pts_seconds)) return failure(media, AVERROR_INVALIDDATA, "invalid video PTS");
    AVRational duration_base = frame->time_base;
    if (duration_base.num <= 0 || duration_base.den <= 0)
        duration_base = media->format->streams[media->stream[0]]->time_base;
    if (duration_base.num <= 0 || duration_base.den <= 0)
        return failure(media, AVERROR_INVALIDDATA, "decoded video duration has no valid time base");
    double period = frame->duration > 0 ? frame->duration * av_q2d(duration_base) : media->frame_period;
    if (!isfinite(period) || period <= 0 || period > 3600.0)
        return failure(media, AVERROR_INVALIDDATA, "invalid decoded video duration");
    media->next_video = output->pts_seconds + period;
    output->reserved = period * 1000000.0 > UINT_MAX ? UINT_MAX : (uint32_t)(period * 1000000.0);
    output->flags |= CS2_MEDIA_VIDEO_DURATION;
    return 1;
}

static int normalize_audio_frame(CS2Media *media) {
    AVFrame *frame = media->frame;
    const AVCodecContext *codec = media->codec[1];
    if (frame->sample_rate <= 0) frame->sample_rate = codec->sample_rate;
    if (frame->ch_layout.nb_channels <= 0 && codec->ch_layout.nb_channels > 0) {
        av_channel_layout_uninit(&frame->ch_layout);
        int status = av_channel_layout_copy(&frame->ch_layout, &codec->ch_layout);
        if (status < 0) return failure(media, status, "restore decoded audio layout");
    }
    /* WMA/AVI PCM may declare only channel count. For one/two channels there
     * is a unique conventional ordering; do not guess multichannel routing.
     */
    if (frame->ch_layout.order == AV_CHANNEL_ORDER_UNSPEC &&
        frame->ch_layout.nb_channels > 0 && frame->ch_layout.nb_channels <= 2) {
        int channels = frame->ch_layout.nb_channels;
        av_channel_layout_uninit(&frame->ch_layout);
        av_channel_layout_default(&frame->ch_layout, channels);
    }
    if (!av_channel_layout_check(&frame->ch_layout))
        return failure(media, AVERROR_INVALIDDATA, "decoded audio channel layout is invalid");
    return 0;
}

static int resampler_open(CS2Media *media) {
    AVFrame *frame = media->frame;
    int rate = frame->sample_rate;
    if (rate <= 0 || rate > 384000 || frame->ch_layout.nb_channels <= 0 ||
        frame->ch_layout.nb_channels > 32)
        return failure(media, AVERROR_INVALIDDATA, "invalid decoded audio format");
    if (media->resample) {
        if (rate != media->audio_rate || frame->format != media->audio_format ||
            av_channel_layout_compare(&frame->ch_layout, &media->audio_layout))
            return failure(media, AVERROR_INVALIDDATA, "audio converter changed without draining");
        return 0;
    }
    AVChannelLayout layout;
    av_channel_layout_default(&layout, OUTPUT_CHANNELS);
    int result = swr_alloc_set_opts2(&media->resample, &layout, AV_SAMPLE_FMT_S16, OUTPUT_RATE,
                                    &frame->ch_layout, (enum AVSampleFormat)frame->format,
                                    rate, 0, NULL);
    av_channel_layout_uninit(&layout);
    if (result < 0) return failure(media, result, "allocate audio resampler");
    result = swr_init(media->resample);
    if (result < 0) return failure(media, result, "initialize audio resampler");
    result = av_channel_layout_copy(&media->audio_layout, &frame->ch_layout);
    if (result < 0) return failure(media, result, "retain audio channel layout");
    media->audio_rate = rate;
    media->audio_format = frame->format;
    return 0;
}

static int audio_frame(CS2Media *media, CS2MediaFrame *output, int flush) {
    if (!flush) {
        int status = resampler_open(media);
        if (status < 0) return status;
    }
    if (!media->resample) return 0;
    int input_frames = flush ? 0 : media->frame->nb_samples;
    if (input_frames < 0 || input_frames > MAX_AUDIO_FRAMES)
        return failure(media, AVERROR_INVALIDDATA, "audio frame exceeds limit");
    int capacity = swr_get_out_samples(media->resample, input_frames);
    if (capacity < 0) return failure(media, capacity, "query resampled audio bound");
    if (!capacity) return 0;
    if (capacity > MAX_AUDIO_FRAMES) return failure(media, AVERROR(ENOMEM), "resampled audio exceeds limit");
    size_t bytes = (size_t)capacity * OUTPUT_CHANNELS * sizeof(int16_t);
    if (bytes > media->pcm_size) {
        uint8_t *replacement = av_realloc(media->pcm, bytes);
        if (!replacement) return failure(media, AVERROR(ENOMEM), "allocate PCM");
        media->pcm = replacement;
        media->pcm_size = bytes;
    }
    int64_t delay = swr_get_delay(media->resample, media->audio_rate);
    double pts = flush ? media->next_audio :
        frame_time(media, 1, media->next_audio + delay / (double)media->audio_rate)
        - delay / (double)media->audio_rate;
    uint8_t *dest[1] = {media->pcm};
    int count = swr_convert(media->resample, dest, capacity,
        flush ? NULL : (const uint8_t **)media->frame->extended_data, input_frames);
    if (count < 0) return failure(media, count, "convert PCM");
    if (!count) return 0;
    if (!isfinite(pts)) return failure(media, AVERROR_INVALIDDATA, "invalid audio PTS");
    if (!flush && media->audio_reconfigured && media->audio_output_seen &&
        pts < media->next_audio) {
        // Retain old converter tail instead of dropping it on reconfiguration.
        // Subsample rounding may overlap its final timestamp; a real backwards
        // discontinuity remains an error, not a silently fabricated clock.
        if (media->next_audio - pts > 0.25)
            return failure(media, AVERROR_INVALIDDATA, "audio format switch has a backwards timestamp discontinuity");
        pts = media->next_audio;
    }
    output->kind = CS2_MEDIA_AUDIO;
    output->flags = media->audio_reconfigured ? CS2_MEDIA_AUDIO_RECONFIGURED : 0u;
    output->sample_frames = (uint32_t)count;
    output->sample_rate = OUTPUT_RATE;
    output->channels = OUTPUT_CHANNELS;
    output->data = media->pcm;
    output->byte_length = (uint64_t)count * OUTPUT_CHANNELS * sizeof(int16_t);
    output->pts_seconds = pts;
    media->next_audio = pts + count / (double)OUTPUT_RATE;
    media->audio_output_seen = 1;
    media->audio_reconfigured = 0;
    return 1;
}

int32_t cs2_media_read(CS2Media *media, CS2MediaFrame *output) {
    if (!media || !output) return AVERROR(EINVAL);
    memset(output, 0, sizeof(*output));
    /* Real demux/decode work only. The bound guards corrupt streams which
     * never produce video/audio, in addition to the I/O interrupt callback. */
    for (unsigned work = 0; work < 1000000; ++work) {
        if (interrupted(media)) return failure(media, AVERROR_EXIT, "media read cancelled");
        if (media->audio_frame_pending) {
            AVFrame *frame = media->frame;
            if (media->resample && (frame->sample_rate != media->audio_rate ||
                frame->format != media->audio_format ||
                av_channel_layout_compare(&frame->ch_layout, &media->audio_layout))) {
                /* Keep the new AVFrame referenced across returns while all
                 * PCM delayed in the OLD format is emitted, then recreate. */
                int result = audio_frame(media, output, 1);
                if (result) return result;
                swr_free(&media->resample);
                av_channel_layout_uninit(&media->audio_layout);
                media->audio_reconfigured = 1;
            }
            int result = audio_frame(media, output, 0);
            if (result >= 0) media->audio_frame_pending = 0;
            if (result) return result;
            continue;
        }
        if (media->receiving >= 0) {
            int slot = media->receiving;
            av_frame_unref(media->frame);
            int result = avcodec_receive_frame(media->codec[slot], media->frame);
            if (!result) {
                media->packet_needs_receive = 0;
                if (slot == 1) {
                    result = normalize_audio_frame(media);
                    if (result < 0) return result;
                    media->audio_frame_pending = 1;
                    continue;
                }
                result = video_frame(media, output);
                if (result) return result;
                continue;
            }
            if (result == AVERROR_EOF) media->drained[slot] = 1;
            else if (result != AVERROR(EAGAIN)) return failure(media, result, "receive decoded frame");
            else if (media->packet_needs_receive)
                return failure(media, AVERROR_INVALIDDATA, "codec send/receive both refused progress");
            media->receiving = -1;
        }
        if (media->pending_packet) {
            int slot = media->packet->stream_index == media->stream[0] ? 0 : 1;
            int result = avcodec_send_packet(media->codec[slot], media->packet);
            if (result == AVERROR(EAGAIN)) {
                media->packet_needs_receive = 1;
                media->receiving = slot;
                continue;
            }
            if (result < 0) return failure(media, result, "send compressed packet");
            av_packet_unref(media->packet);
            media->pending_packet = 0;
            media->packet_needs_receive = 0;
            media->receiving = slot;
            continue;
        }
        if (media->demux_ended) {
            int scheduled = 0;
            for (int slot = 0; slot < 2; ++slot) {
                if (media->drained[slot]) continue;
                if (!media->flush_sent[slot]) {
                    int result = avcodec_send_packet(media->codec[slot], NULL);
                    if (result < 0 && result != AVERROR(EAGAIN) && result != AVERROR_EOF)
                        return failure(media, result, "flush codec");
                    if (result != AVERROR(EAGAIN)) media->flush_sent[slot] = 1;
                    else media->packet_needs_receive = 1;
                }
                media->receiving = slot;
                scheduled = 1;
                break;
            }
            if (scheduled) continue;
            if (!media->resample_drained) {
                int result = audio_frame(media, output, 1);
                if (result) return result;
                media->resample_drained = 1;
            }
            return 0;
        }
        int result = av_read_frame(media->format, media->packet);
        if (result == AVERROR_EOF) {
            if (media->avio->error < 0 && media->avio->error != AVERROR_EOF)
                return failure(media, media->avio->error, "archive AVIO ended with a read error");
            media->demux_ended = 1;
            continue;
        }
        if (result < 0) return failure(media, result, "read compressed packet");
        if ((media->packet->stream_index == media->stream[0] && (media->decode_mask & 1u)) ||
            (media->packet->stream_index == media->stream[1] && (media->decode_mask & 2u)))
            media->pending_packet = 1;
        else av_packet_unref(media->packet);
    }
    return failure(media, AVERROR_INVALIDDATA, "media work limit reached without output");
}
