#ifndef FUGUE_H
#define FUGUE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct FugueEngine FugueEngine;

FugueEngine *fugue_engine_new(uint32_t sample_rate);
void fugue_engine_free(FugueEngine *engine);

int32_t fugue_engine_load_json(FugueEngine *engine, const uint8_t *json, size_t json_len);
int32_t fugue_engine_reset(FugueEngine *engine);

size_t fugue_engine_render_interleaved(
    FugueEngine *engine,
    float *output,
    size_t frame_count
);

int32_t fugue_engine_set_control_number(
    FugueEngine *engine,
    const char *module_id,
    const char *key,
    float value
);

int32_t fugue_engine_set_control_bool(
    FugueEngine *engine,
    const char *module_id,
    const char *key,
    bool value
);

int32_t fugue_engine_set_control_string(
    FugueEngine *engine,
    const char *module_id,
    const char *key,
    const char *value
);

const char *fugue_engine_last_error(const FugueEngine *engine);
void fugue_engine_clear_error(FugueEngine *engine);

#ifdef __cplusplus
}
#endif

#endif
