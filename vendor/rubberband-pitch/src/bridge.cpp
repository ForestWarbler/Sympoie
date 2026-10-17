#include "../upstream/rubberband/RubberBandStretcher.h"

#include <algorithm>
#include <cstdio>
#include <exception>
#include <memory>
#include <stdexcept>

using Stretcher = RubberBand::RubberBandStretcher;

namespace {
void report(char *error, size_t capacity, const char *message) noexcept {
    if (capacity) std::snprintf(error, capacity, "%s", message);
}
}

extern "C" {
void *sympoie_rb3_create(unsigned int sample_rate, double pitch_scale,
                         int preserve_formants, size_t duration,
                         size_t block_size, char *error, size_t capacity) noexcept {
    try {
        auto options = Stretcher::OptionProcessOffline |
            Stretcher::OptionEngineFiner | Stretcher::OptionChannelsTogether |
            Stretcher::OptionPitchHighQuality | Stretcher::OptionThreadingNever;
        if (preserve_formants) options |= Stretcher::OptionFormantPreserved;
        std::unique_ptr<Stretcher> stretcher(new Stretcher(
            sample_rate, 2, options, 1.0, pitch_scale));
        if (stretcher->getEngineVersion() != 3) {
            throw std::runtime_error("Rubber Band did not select the requested R3 engine");
        }
        stretcher->setMaxProcessSize(block_size);
        stretcher->setExpectedInputDuration(duration);
        return stretcher.release();
    } catch (const std::exception &e) {
        report(error, capacity, e.what());
    } catch (...) {
        report(error, capacity, "Unknown C++ error creating Rubber Band R3");
    }
    return nullptr;
}

int sympoie_rb3_feed(void *handle, const float *left, const float *right,
                     size_t count, int final_block, int study,
                     char *error, size_t capacity) noexcept {
    try {
        const float *input[2] = {left, right};
        auto *stretcher = static_cast<Stretcher *>(handle);
        if (study) stretcher->study(input, count, final_block != 0);
        else stretcher->process(input, count, final_block != 0);
        return 0;
    } catch (const std::exception &e) {
        report(error, capacity, e.what());
    } catch (...) {
        report(error, capacity, "Unknown C++ error processing Rubber Band R3 audio");
    }
    return -1;
}

int sympoie_rb3_retrieve(void *handle, float *left, float *right,
                         size_t max_count, char *error, size_t capacity) noexcept {
    try {
        auto *stretcher = static_cast<Stretcher *>(handle);
        int available = stretcher->available();
        if (available <= 0) return available;
        float *output[2] = {left, right};
        size_t count = stretcher->retrieve(output, std::min(max_count, size_t(available)));
        if (!count) throw std::runtime_error("Rubber Band R3 could not retrieve available audio");
        return int(count);
    } catch (const std::exception &e) {
        report(error, capacity, e.what());
    } catch (...) {
        report(error, capacity, "Unknown C++ error retrieving Rubber Band R3 audio");
    }
    return -2;
}

void sympoie_rb3_destroy(void *handle) noexcept {
    try { delete static_cast<Stretcher *>(handle); } catch (...) { }
}
}
