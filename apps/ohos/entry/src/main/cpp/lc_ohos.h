#ifndef LITECODE_LC_OHOS_H
#define LITECODE_LC_OHOS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// docs/plans/ohos-shell.md §5.3 C ABI.
typedef void (*lc_event_cb)(const char* json, void* user);

uint64_t lc_ohos_init(const char* config_json);
int32_t lc_ohos_call(uint64_t handle, const char* cmd, const char* args_json,
                     char** out_json, char** out_error);
void lc_ohos_set_event_sink(uint64_t handle, lc_event_cb cb, void* user);
void lc_ohos_dispose(uint64_t handle);
void lc_ohos_free(char* ptr);

#ifdef __cplusplus
}
#endif

#endif // LITECODE_LC_OHOS_H
