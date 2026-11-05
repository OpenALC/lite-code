#include "napi/native_api.h"

// Spike: 模拟 CoreHandle 通过 NAPI 跨语言传递。真实实现会在这里包装
// Rust CoreHandle 的指针/注册表 id，并暴露 request/cancel 等方法。
static napi_value CreateHandle(napi_env env, napi_callback_info info)
{
    napi_value object;
    napi_create_object(env, &object);

    napi_value id;
    napi_create_string_utf8(env, "core-handle-42", NAPI_AUTO_LENGTH, &id);
    napi_set_named_property(env, object, "id", id);

    napi_value native;
    napi_get_boolean(env, true, &native);
    napi_set_named_property(env, object, "native", native);

    return object;
}

EXTERN_C_START
static napi_value Init(napi_env env, napi_value exports)
{
    napi_property_descriptor desc[] = {
        { "createHandle", nullptr, CreateHandle, nullptr, nullptr, nullptr, napi_default, nullptr }
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
