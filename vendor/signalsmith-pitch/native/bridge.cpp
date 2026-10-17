#include "signalsmith-stretch.h"
#include <cstdio>
#include <exception>
#include <memory>

namespace {
using Stretch = signalsmith::stretch::SignalsmithStretch<float>;

struct Input {
    const float *samples;
    size_t frames;
    struct Channel {
        const float *samples;
        size_t frames;
        int channel;
        float operator[](int index) const {
            return size_t(index) < frames ? samples[size_t(index)*2 + channel] : 0;
        }
    };
    Channel operator[](int channel) const { return {samples, frames, channel}; }
};
struct Output {
    float *samples;
    struct Channel {
        float *samples;
        float &operator[](int index) const { return samples[size_t(index)*2]; }
    };
    Channel operator[](int channel) const { return {samples + channel}; }
};

template<class Function>
int guarded(Function &&function, char *error, size_t capacity) noexcept {
    try {
        function();
        return 1;
    } catch (const std::exception &e) {
        std::snprintf(error, capacity, "Signalsmith Stretch: %s", e.what());
    } catch (...) {
        std::snprintf(error, capacity, "Signalsmith Stretch: unknown native exception");
    }
    return 0;
}
}

extern "C" {
void *sympoie_ss_create(unsigned sample_rate, float semitones, int preserve_formants,
                       int *seek_frames, int *tail_frames, char *error, size_t capacity) noexcept {
    Stretch *result = nullptr;
    guarded([&] {
        auto stretch = std::unique_ptr<Stretch>(new Stretch(0));
        stretch->presetDefault(2, float(sample_rate));
        stretch->setTransposeSemitones(semitones);
        stretch->setFormantFactor(1, preserve_formants != 0);
        stretch->setFormantBase(0);
        *seek_frames = stretch->outputSeekLength(1);
        *tail_frames = stretch->intervalSamples();
        result = stretch.release();
    }, error, capacity);
    return result;
}

int sympoie_ss_prime(void *handle, const float *input, size_t available, int seek_frames,
                    char *error, size_t capacity) noexcept {
    return guarded([&] {
        static_cast<Stretch *>(handle)->outputSeek(Input{input, available}, seek_frames);
    }, error, capacity);
}

int sympoie_ss_process(void *handle, const float *input, size_t available,
                      float *output, int frames, char *error, size_t capacity) noexcept {
    return guarded([&] {
        static_cast<Stretch *>(handle)->process(Input{input, available}, frames, Output{output}, frames);
    }, error, capacity);
}

int sympoie_ss_flush(void *handle, float *output, int frames, char *error, size_t capacity) noexcept {
    return guarded([&] {
        static_cast<Stretch *>(handle)->flush(Output{output}, frames, 1);
    }, error, capacity);
}

void sympoie_ss_destroy(void *handle) noexcept {
    delete static_cast<Stretch *>(handle);
}
}
