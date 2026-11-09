#include "napi/native_api.h"

#include <hilog/log.h>

#include <cstring>
#include <mutex>
#include <string>
#include <vector>

#include "lc_ohos.h"

// NAPI bridge for the HarmonyOS shell (docs/plans/ohos-shell.md §5.3).
//
// `invoke` runs `lc_ohos_call` on a worker thread via napi async work; events
// from Rust reach ArkTS through a threadsafe function, then NativeBridge.ets
// injects them into the WebView with runJavaScript.
namespace {

constexpr unsigned int kLogDomain = 0x0000;
constexpr const char* kLogTag = "NativeBridge";

std::mutex g_mutex;
uint64_t g_handle = 0;
napi_threadsafe_function g_sink = nullptr;

std::string Utf8Arg(napi_env env, napi_value value, bool& ok)
{
    ok = false;
    napi_valuetype type = napi_undefined;
    if (napi_typeof(env, value, &type) != napi_ok || type != napi_string) {
        return {};
    }
    size_t length = 0;
    if (napi_get_value_string_utf8(env, value, nullptr, 0, &length) != napi_ok) {
        return {};
    }
    std::vector<char> buffer(length + 1, '\0');
    size_t copied = 0;
    if (napi_get_value_string_utf8(env, value, buffer.data(), buffer.size(), &copied) != napi_ok) {
        return {};
    }
    ok = true;
    return std::string(buffer.data(), copied);
}

// Runs on the ArkTS/UI thread. `data` is a heap copy made in OnRustEvent.
void CallJsSink(napi_env env, napi_value js_callback, void* context, void* data)
{
    (void)context;
    char* json = static_cast<char*>(data);
    if (env != nullptr && js_callback != nullptr && json != nullptr) {
        napi_value undefined = nullptr;
        napi_value argument = nullptr;
        napi_get_undefined(env, &undefined);
        napi_create_string_utf8(env, json, NAPI_AUTO_LENGTH, &argument);
        napi_call_function(env, undefined, js_callback, 1, &argument, nullptr);
    }
    delete[] json;
}

// Runs on an arbitrary Rust worker thread. The envelope pointer is owned by us.
void OnRustEvent(const char* json, void* user)
{
    (void)user;
    if (json == nullptr) {
        return;
    }
    std::lock_guard<std::mutex> lock(g_mutex);
    if (g_sink == nullptr) {
        lc_ohos_free(const_cast<char*>(json));
        return;
    }
    const size_t size = std::strlen(json) + 1;
    char* copy = new (std::nothrow) char[size];
    if (copy == nullptr) {
        lc_ohos_free(const_cast<char*>(json));
        return;
    }
    std::memcpy(copy, json, size);
    lc_ohos_free(const_cast<char*>(json));

    if (napi_call_threadsafe_function(g_sink, copy, napi_tsfn_nonblocking) != napi_ok) {
        delete[] copy;
    }
}

void Cleanup(void* arg)
{
    (void)arg;
    std::lock_guard<std::mutex> lock(g_mutex);
    if (g_sink != nullptr) {
        napi_release_threadsafe_function(g_sink, napi_tsfn_abort);
        g_sink = nullptr;
    }
    if (g_handle != 0) {
        lc_ohos_dispose(g_handle);
        g_handle = 0;
    }
}

napi_value GetBridgeVersion(napi_env env, napi_callback_info info)
{
    (void)info;
    napi_value version = nullptr;
    napi_create_string_utf8(env, "lc-ohos-napi", NAPI_AUTO_LENGTH, &version);
    return version;
}

napi_value InitHost(napi_env env, napi_callback_info info)
{
    size_t argc = 1;
    napi_value argv[1] = {nullptr};
    napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr);
    bool ok = false;
    std::string config = argc > 0 ? Utf8Arg(env, argv[0], ok) : std::string();
    if (!ok) {
        napi_throw_type_error(env, nullptr, "init(configJson: string) expects a JSON string");
        return nullptr;
    }

    std::lock_guard<std::mutex> lock(g_mutex);
    if (g_handle != 0) {
        lc_ohos_dispose(g_handle);
        g_handle = 0;
    }
    g_handle = lc_ohos_init(config.c_str());
    if (g_handle == 0) {
        napi_throw_error(env, nullptr, "lc_ohos_init failed; see hilog for details");
        return nullptr;
    }
    lc_ohos_set_event_sink(g_handle, OnRustEvent, nullptr);
    OH_LOG_Print(LOG_APP, LOG_INFO, kLogDomain, kLogTag, "host initialized");
    napi_value result = nullptr;
    napi_create_int32(env, 1, &result);
    return result;
}

napi_value DisposeHost(napi_env env, napi_callback_info info)
{
    (void)info;
    std::lock_guard<std::mutex> lock(g_mutex);
    if (g_sink != nullptr) {
        napi_release_threadsafe_function(g_sink, napi_tsfn_abort);
        g_sink = nullptr;
    }
    if (g_handle != 0) {
        lc_ohos_dispose(g_handle);
        g_handle = 0;
    }
    return nullptr;
}

napi_value SetEventSink(napi_env env, napi_callback_info info)
{
    size_t argc = 1;
    napi_value argv[1] = {nullptr};
    napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr);
    napi_valuetype type = napi_undefined;
    if (argc < 1 || napi_typeof(env, argv[0], &type) != napi_ok || type != napi_function) {
        napi_throw_type_error(env, nullptr, "setEventSink(callback) expects a function");
        return nullptr;
    }

    napi_value resource_name = nullptr;
    napi_create_string_utf8(env, "lcOhosEvent", NAPI_AUTO_LENGTH, &resource_name);
    napi_threadsafe_function sink = nullptr;
    napi_status status = napi_create_threadsafe_function(
        env, argv[0], nullptr, resource_name, 0, 1, nullptr, nullptr, nullptr, CallJsSink, &sink);
    if (status != napi_ok) {
        napi_throw_error(env, nullptr, "napi_create_threadsafe_function failed");
        return nullptr;
    }

    std::lock_guard<std::mutex> lock(g_mutex);
    if (g_sink != nullptr) {
        napi_release_threadsafe_function(g_sink, napi_tsfn_abort);
    }
    g_sink = sink;
    return nullptr;
}

struct CallWork {
    napi_async_work work = nullptr;
    napi_deferred deferred = nullptr;
    uint64_t handle = 0;
    std::string cmd;
    std::string args;
    std::string result;
    std::string error;
    int32_t status = -1;
};

void ExecuteCall(napi_env env, void* data)
{
    (void)env;
    CallWork* work = static_cast<CallWork*>(data);
    char* out_json = nullptr;
    char* out_error = nullptr;
    work->status = lc_ohos_call(work->handle, work->cmd.c_str(), work->args.c_str(), &out_json, &out_error);
    if (out_json != nullptr) {
        work->result.assign(out_json);
        lc_ohos_free(out_json);
    }
    if (out_error != nullptr) {
        work->error.assign(out_error);
        lc_ohos_free(out_error);
    }
    if (work->status != 0 && work->error.empty()) {
        work->error = "{\"code\":\"internal\",\"message\":\"native call failed\",\"detail\":null}";
    }
}

void CompleteCall(napi_env env, napi_status status, void* data)
{
    (void)status;
    CallWork* work = static_cast<CallWork*>(data);
    if (work->status == 0) {
        napi_value value = nullptr;
        napi_create_string_utf8(env, work->result.c_str(), NAPI_AUTO_LENGTH, &value);
        napi_resolve_deferred(env, work->deferred, value);
    } else {
        napi_value value = nullptr;
        napi_create_string_utf8(env, work->error.c_str(), NAPI_AUTO_LENGTH, &value);
        napi_reject_deferred(env, work->deferred, value);
    }
    napi_delete_async_work(env, work->work);
    delete work;
}

napi_value Invoke(napi_env env, napi_callback_info info)
{
    size_t argc = 2;
    napi_value argv[2] = {nullptr, nullptr};
    napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr);
    if (argc < 2) {
        napi_throw_type_error(env, nullptr, "invoke(cmd: string, argsJson: string) expects two strings");
        return nullptr;
    }
    bool ok_cmd = false;
    bool ok_args = false;
    std::string cmd = Utf8Arg(env, argv[0], ok_cmd);
    std::string args = Utf8Arg(env, argv[1], ok_args);
    if (!ok_cmd || !ok_args) {
        napi_throw_type_error(env, nullptr, "invoke arguments must be strings");
        return nullptr;
    }

    CallWork* work = new (std::nothrow) CallWork();
    if (work == nullptr) {
        napi_throw_error(env, nullptr, "out of memory");
        return nullptr;
    }
    {
        std::lock_guard<std::mutex> lock(g_mutex);
        work->handle = g_handle;
    }
    if (work->handle == 0) {
        delete work;
        napi_throw_error(env, nullptr, "native host is not initialized");
        return nullptr;
    }
    work->cmd = std::move(cmd);
    work->args = std::move(args);

    napi_value promise = nullptr;
    napi_create_promise(env, &work->deferred, &promise);
    napi_value resource_name = nullptr;
    napi_create_string_utf8(env, "lcOhosCall", NAPI_AUTO_LENGTH, &resource_name);
    napi_status status = napi_create_async_work(
        env, nullptr, resource_name, ExecuteCall, CompleteCall, work, &work->work);
    if (status != napi_ok) {
        napi_delete_async_work(env, work->work);
        delete work;
        napi_throw_error(env, nullptr, "napi_create_async_work failed");
        return nullptr;
    }
    status = napi_queue_async_work(env, work->work);
    if (status != napi_ok) {
        napi_delete_async_work(env, work->work);
        delete work;
        napi_throw_error(env, nullptr, "napi_queue_async_work failed");
        return nullptr;
    }
    return promise;
}

napi_value Init(napi_env env, napi_value exports)
{
    napi_property_descriptor desc[] = {
        {"getBridgeVersion", nullptr, GetBridgeVersion, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"init", nullptr, InitHost, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"invoke", nullptr, Invoke, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"setEventSink", nullptr, SetEventSink, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"dispose", nullptr, DisposeHost, nullptr, nullptr, nullptr, napi_default, nullptr},
    };
    napi_define_properties(env, exports, sizeof(desc) / sizeof(desc[0]), desc);
    napi_add_env_cleanup_hook(env, Cleanup, nullptr);
    return exports;
}

} // namespace

static napi_module entryModule = {
    .nm_version = 1,
    .nm_flags = 0,
    .nm_filename = nullptr,
    .nm_register_func = Init,
    .nm_modname = "entry",
    .nm_priv = ((void*)0),
    .reserved = {0},
};

extern "C" __attribute__((constructor)) void RegisterEntryModule(void)
{
    napi_module_register(&entryModule);
}
