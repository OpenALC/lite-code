#include "napi/native_api.h"

// Placeholder for the Phase 3 NAPI bridge. The real implementation will wrap
// the Rust host in crates/lc-ohos; this module only verifies that the native
// entry point builds and loads.
static napi_value GetBridgeVersion(napi_env env, napi_callback_info info)
{
    napi_value version;
    napi_create_string_utf8(env, "lc-ohos-placeholder", NAPI_AUTO_LENGTH, &version);
    return version;
}

EXTERN_C_START
static napi_value Init(napi_env env, napi_value exports)
{
    napi_property_descriptor desc[] = {
        { "getBridgeVersion", nullptr, GetBridgeVersion, nullptr, nullptr, nullptr, napi_default, nullptr }
    };
    napi_define_properties(env, exports, sizeof(desc) / sizeof(desc[0]), desc);
    return exports;
}
EXTERN_C_END

static napi_module entryModule = {
    .nm_version = 1,
    .nm_flags = 0,
    .nm_filename = nullptr,
    .nm_register_func = Init,
    .nm_modname = "entry",
    .nm_priv = ((void*)0),
    .reserved = { 0 },
};

extern "C" __attribute__((constructor)) void RegisterEntryModule(void)
{
    napi_module_register(&entryModule);
}
