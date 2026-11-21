#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static uint64_t monotonic_ns(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) == -1) {
        perror("CLOCK_MONOTONIC");
        exit(EXIT_FAILURE);
    }
    return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}

static int capture_png(const char *filename, uint64_t *start, uint64_t *end) {
    *start = monotonic_ns();
    pid_t child = fork();
    if (child == -1) {
        perror("fork screencap");
        return -1;
    }
    if (child == 0) {
        execl("/system/bin/screencap", "screencap", "-p", filename, (char *)NULL);
        perror("exec screencap");
        _exit(127);
    }

    int status;
    pid_t waited;
    do {
        waited = waitpid(child, &status, 0);
    } while (waited == -1 && errno == EINTR);
    *end = monotonic_ns();
    if (waited == -1) {
        perror("waitpid screencap");
        return -1;
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        fprintf(stderr, "screencap failed for %s (wait status %d)\n", filename, status);
        return -1;
    }

    struct stat file;
    if (stat(filename, &file) == -1 || !S_ISREG(file.st_mode) || file.st_size <= 0) {
        fprintf(stderr, "missing or empty PNG: %s\n", filename);
        return -1;
    }
    return 0;
}

static long parse_number(const char *value, long min, long max) {
    char *end;
    errno = 0;
    long number = strtol(value, &end, 10);
    return errno || end == value || *end || number < min || number > max ? -1 : number;
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "Usage: %s NEW_DIRECTORY COUNT[2..60] DELAY_MS[0..1000]\n", argv[0]);
        return EXIT_FAILURE;
    }
    const long count = parse_number(argv[2], 2, 60);
    const long delay_ms = parse_number(argv[3], 0, 1000);
    if (count < 0 || delay_ms < 0) {
        fprintf(stderr, "COUNT must be 2..60 and DELAY_MS must be 0..1000\n");
        return EXIT_FAILURE;
    }
    if (mkdir(argv[1], 0700) == -1 || chdir(argv[1]) == -1) {
        perror(argv[1]);
        return EXIT_FAILURE;
    }
    FILE *timings = fopen("capture.csv.partial", "w");
    if (!timings) {
        perror("capture.csv.partial");
        return EXIT_FAILURE;
    }
    if (fputs("seq,png,start_ns,end_ns\n", timings) == EOF) {
        perror("write capture metadata");
        return EXIT_FAILURE;
    }
    for (long sequence = 1; sequence <= count; ++sequence) {
        char filename[32];
        snprintf(filename, sizeof(filename), "frame_%03ld.png", sequence);
        uint64_t start, end;
        if (capture_png(filename, &start, &end) == -1) {
            return EXIT_FAILURE;
        }
        if (fprintf(timings, "%ld,%s,%" PRIu64 ",%" PRIu64 "\n", sequence, filename, start, end) < 0 ||
            fflush(timings) == EOF) {
            perror("write capture metadata");
            return EXIT_FAILURE;
        }
        if (sequence == 1) {
            puts("READY: baseline saved; tap once in the game");
            fflush(stdout);
        }
        if (delay_ms && sequence < count) {
            struct timespec delay = { .tv_sec = delay_ms / 1000, .tv_nsec = (delay_ms % 1000) * 1000000L };
            int slept;
            do {
                slept = nanosleep(&delay, &delay);
            } while (slept == -1 && errno == EINTR);
            if (slept == -1) {
                perror("screencap interval");
                return EXIT_FAILURE;
            }
        }
    }
    if (fclose(timings) == EOF || rename("capture.csv.partial", "capture.csv") == -1) {
        perror("finalize capture.csv");
        return EXIT_FAILURE;
    }
    printf("SAVED: %ld screenshots and capture.csv\n", count);
    return EXIT_SUCCESS;
}
