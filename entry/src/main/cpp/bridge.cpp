#include <ace/xcomponent/native_interface_xcomponent.h>
#include <native_window/external_window.h>
#include <native_buffer/native_buffer.h>
#include <native_vsync/native_vsync.h>
#include <napi/native_api.h>
#include <hilog/log.h>
#include <arkui/ui_input_event.h>
#include <atomic>
#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <set>
#include <string>
#include <vector>
#include "display_color.h"
#include <sys/mman.h>
#include <unistd.h>

// Called exclusively on the Rust render worker, after the surface is created/configured.
// Read back the declaration so the editor never emits P3 into an untagged surface.
extern "C" int32_t gc_set_window_color_space(void* window, bool p3) {
    auto* nativeWindow = static_cast<OHNativeWindow*>(window);
    const auto requested = p3 ? OH_COLORSPACE_P3_FULL : OH_COLORSPACE_SRGB_FULL;
    int32_t result = OH_NativeWindow_SetColorSpace(nativeWindow, requested);
    OH_NativeBuffer_ColorSpace actual = OH_COLORSPACE_NONE;
    if (result == 0) result = OH_NativeWindow_GetColorSpace(nativeWindow, &actual);
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft",
        "Native output requested %{public}d actual %{public}d result %{public}d",
        static_cast<int>(requested), static_cast<int>(actual), result);
    return result != 0 ? result : (GridCraftOutputMatches(p3, actual) ? 0 : -1);
}

// Rust `log` crate 的出口：wgpu/wgpu-hal 的诊断（EGL 平台选择、surface 创建、swap 失败）
// 只有接上 hilog 才能在设备上看到，否则 log::error! 会被静默丢弃。
extern "C" void gc_hilog(int32_t level, const char* message) {
    const LogLevel priority = level >= 2 ? LOG_ERROR : (level == 1 ? LOG_WARN : LOG_INFO);
    OH_LOG_Print(LOG_APP, priority, 0xD001, "GridCraftGL", "%{public}s", message ? message : "");
}

extern "C" {
using Notify = void (*)(uint64_t,const uint8_t*,size_t,const uint8_t*,size_t);
bool gc_init(const uint8_t*,size_t,Notify);
void gc_attach(uintptr_t,uint32_t,uint32_t);
void gc_resize(uint32_t,uint32_t);
void gc_detach();
void gc_frame();
void gc_active(bool);
void gc_close();
void gc_input(const uint8_t*,size_t);
void gc_reply(uint64_t,bool,const uint8_t*,size_t,const uint8_t*,size_t);
void gc_open(const uint8_t*,size_t,const uint8_t*,size_t);
void gc_drop_file(const uint8_t*,size_t,int32_t,float,float);
void gc_drop(const uint8_t*,size_t,const uint8_t*,size_t,float,float);
void gc_command(const uint8_t*,size_t);
}
namespace {
std::mutex notifyMutex;
napi_threadsafe_function notifyFn = nullptr;
OH_NativeVSync* vsync = nullptr;
std::atomic<bool> framePending(false);
std::atomic<bool> attached(false);
// Single main window for v1 (sub-windows are not wired yet).
struct WinState {
    uint32_t key = 0;
    std::atomic<int32_t> touchId{-1};
};
std::mutex winMutex;
std::map<OH_NativeXComponent*,std::shared_ptr<WinState>> windows;
thread_local WinState* cur = nullptr;
WinState* Enter(OH_NativeXComponent* c) {
    std::lock_guard<std::mutex> lock(winMutex);
    auto it = windows.find(c);
    cur = it == windows.end() ? nullptr : it->second.get();
    return cur;
}
struct Request { uint64_t id; std::string json; std::vector<uint8_t> data; };
// Consumed and freed on the ArkTS thread; borrowed slices in the Rust callback are copied by NotifyRust.
void CallJS(napi_env env, napi_value callback, void*, void* ptr) {
    auto* r = static_cast<Request*>(ptr);
    if (env && callback) {
        napi_value args[3], result, receiver;
        napi_create_double(env, static_cast<double>(r->id), &args[0]);
        napi_create_string_utf8(env, r->json.data(), r->json.size(), &args[1]);
        void* data = nullptr;
        napi_create_arraybuffer(env, r->data.size(), &data, &args[2]);
        if (!r->data.empty()) std::memcpy(data, r->data.data(), r->data.size());
        napi_get_undefined(env, &receiver);
        if (napi_call_function(env, receiver, callback, 3, args, &result) != napi_ok && r->id) {
            const std::string error = "platform callback failed";
            gc_reply(r->id, false, reinterpret_cast<const uint8_t*>(error.data()), error.size(), nullptr, 0);
        }
    } else if (r->id) {
        const std::string error = "cancelled";
        gc_reply(r->id, false, reinterpret_cast<const uint8_t*>(error.data()), error.size(), nullptr, 0);
    }
    delete r;
}
void NotifyRust(uint64_t id, const uint8_t* json, size_t n, const uint8_t* data, size_t size) {
    // The Rust worker never calls JS directly; platform requests travel through a thread-safe queue.
    auto* r = new Request{id, std::string(reinterpret_cast<const char*>(json), n), {}};
    if (r->json.find("\"kind\":\"ime\"") == std::string::npos)
        OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft", "Rust notification %{public}s", r->json.c_str());
    if (size) r->data.assign(data, data + size);
    std::lock_guard<std::mutex> lock(notifyMutex);
    if (!notifyFn || napi_call_threadsafe_function(notifyFn, r, napi_tsfn_nonblocking) != napi_ok) {
        delete r;
        if (id) { const std::string error = "cancelled"; gc_reply(id, false, reinterpret_cast<const uint8_t*>(error.data()), error.size(), nullptr, 0); }
    }
}
// VSync only fires a frame; egui updates and GPU drawing still run serially on the Rust worker.
void Vsync(long long, void*) { framePending = false; if (attached) gc_frame(); }
std::string String(napi_env env, napi_value v) {
    size_t len = 0;
    if (!v || napi_get_value_string_utf8(env, v, nullptr, 0, &len) != napi_ok || len > 1024 * 1024) return {};
    std::vector<char> data(len + 1);
    if (napi_get_value_string_utf8(env, v, data.data(), data.size(), &len) != napi_ok) return {};
    return std::string(data.data(), len);
}
napi_value Undefined(napi_env env) { napi_value v; napi_get_undefined(env, &v); return v; }
void Json(const std::string& s) {
    gc_input(reinterpret_cast<const uint8_t*>(s.data()), s.size());
}
// 诊断专用：绕过 EGL，直接向 NativeWindow 写一整帧纯绿。
// 用来把"内容不上屏"分离成 NativeWindow 通路问题 或 EGL/wgpu 问题。
static void SelfTestFillGreen(OHNativeWindow* w, uint32_t width, uint32_t height) {
    OHNativeWindowBuffer* buffer = nullptr;
    int fenceFd = -1;
    int32_t rc = OH_NativeWindow_NativeWindowRequestBuffer(w, &buffer, &fenceFd);
    if (rc != 0 || buffer == nullptr) {
        OH_LOG_Print(LOG_APP, LOG_ERROR, 0xD001, "GridCraft", "selftest request failed rc=%{public}d", rc);
        return;
    }
    BufferHandle* handle = OH_NativeWindow_GetBufferHandleFromNative(buffer);
    if (handle == nullptr || handle->size <= 0 || handle->fd < 0) {
        OH_LOG_Print(LOG_APP, LOG_ERROR, 0xD001, "GridCraft", "selftest no usable handle");
        OH_NativeWindow_NativeWindowAbortBuffer(w, buffer);
        return;
    }
    void* base = mmap(nullptr, static_cast<size_t>(handle->size), PROT_READ | PROT_WRITE, MAP_SHARED, handle->fd, 0);
    if (base == MAP_FAILED) {
        OH_LOG_Print(LOG_APP, LOG_ERROR, 0xD001, "GridCraft", "selftest mmap failed");
        OH_NativeWindow_NativeWindowAbortBuffer(w, buffer);
        return;
    }
    auto* pixels = static_cast<uint32_t*>(base);
    const size_t count = static_cast<size_t>(handle->size) / sizeof(uint32_t);
    for (size_t i = 0; i < count; ++i) pixels[i] = 0xFF00FF00u;
    munmap(base, static_cast<size_t>(handle->size));
    Region region{nullptr, 0};
    rc = OH_NativeWindow_NativeWindowFlushBuffer(w, buffer, fenceFd, region);
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft",
        "selftest flush rc=%{public}d buffer %{public}dx%{public}d stride %{public}d size %{public}d format %{public}d want %{public}u x %{public}u",
        rc, handle->width, handle->height, handle->stride, handle->size, handle->format,
        width, height);
}

void SurfaceCreated(OH_NativeXComponent* c, void* window) {
    auto* state = Enter(c);
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft", "Surface created window=%{public}u", state ? state->key : 0u);
    if (!state) {
        OH_LOG_Print(LOG_APP, LOG_ERROR, 0xD001, "GridCraft",
            "SurfaceCreated with no WinState (registerWindow missing or too late)");
        return;
    }
    uint64_t w = 0, h = 0;
    if (!window || OH_NativeXComponent_GetXComponentSize(c, window, &w, &h) != OH_NATIVEXCOMPONENT_RESULT_SUCCESS) {
        OH_LOG_Print(LOG_APP, LOG_ERROR, 0xD001, "GridCraft", "SurfaceCreated size failed window=%{public}u", state->key);
        return;
    }
    auto ref = OH_NativeWindow_NativeObjectReference(window);
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft", "Window %{public}llu x %{public}llu reference %{public}d key=%{public}u",
        static_cast<unsigned long long>(w), static_cast<unsigned long long>(h), ref, state->key);
    if (ref != 0) {
        OH_LOG_Print(LOG_APP, LOG_ERROR, 0xD001, "GridCraft", "NativeObjectReference failed %{public}d key=%{public}u", ref, state->key);
        return;
    }
    // EGL swaps into whatever geometry the NativeWindow advertises. The XComponent surface arrives
    // without it in some layouts, and then every frame is swapped into a buffer nobody composites —
    // the screen stays black while the renderer reports success. Pin it before wgpu creates the
    // EGL surface.
    int32_t geoBefore[2] = {0, 0};
    auto* nativeWindow = static_cast<OHNativeWindow*>(window);
    OH_NativeWindow_NativeWindowHandleOpt(nativeWindow, GET_BUFFER_GEOMETRY, &geoBefore[0], &geoBefore[1]);
    int32_t geoSet = OH_NativeWindow_NativeWindowHandleOpt(nativeWindow, SET_BUFFER_GEOMETRY,
        static_cast<int32_t>(w), static_cast<int32_t>(h));
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft",
        "Native window geometry before %{public}d x %{public}d, set %{public}llu x %{public}llu rc %{public}d",
        geoBefore[0], geoBefore[1], static_cast<unsigned long long>(w),
        static_cast<unsigned long long>(h), geoSet);
    // GPU 渲染要求 buffer usage 含 HW_RENDER；只有 CPU 标志时，eglSwapBuffers 提交的 buffer
    // 合成器读不到（表现为渲染全成功、屏幕全黑，而 CPU 直接 mmap 写入却能显示）。
    uint64_t usageBefore = 0;
    OH_NativeWindow_NativeWindowHandleOpt(nativeWindow, GET_USAGE, &usageBefore);
    if ((usageBefore & NATIVEBUFFER_USAGE_HW_RENDER) == 0) {
        const uint64_t want = usageBefore | NATIVEBUFFER_USAGE_HW_RENDER | NATIVEBUFFER_USAGE_HW_TEXTURE;
        const int32_t usageRc = OH_NativeWindow_NativeWindowHandleOpt(nativeWindow, SET_USAGE, want);
        OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft",
            "Native window usage 0x%{public}llx -> 0x%{public}llx rc %{public}d",
            static_cast<unsigned long long>(usageBefore), static_cast<unsigned long long>(want), usageRc);
    } else {
        OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft",
            "Native window usage 0x%{public}llx already supports HW_RENDER",
            static_cast<unsigned long long>(usageBefore));
    }
    // 诊断开关：需要单独验证 NativeWindow 通路时取消下行注释（会与 EGL 抢 buffer）。
    // SelfTestFillGreen(nativeWindow, static_cast<uint32_t>(w), static_cast<uint32_t>(h));
    attached = true;
    gc_attach(reinterpret_cast<uintptr_t>(window), static_cast<uint32_t>(w), static_cast<uint32_t>(h));
}
void SurfaceChanged(OH_NativeXComponent* c, void* window) {
    auto* state = Enter(c);
    if (!state) return;
    uint64_t w = 0, h = 0;
    if (OH_NativeXComponent_GetXComponentSize(c, window, &w, &h) != OH_NATIVEXCOMPONENT_RESULT_SUCCESS) return;
    gc_resize(static_cast<uint32_t>(w), static_cast<uint32_t>(h));
}
void SurfaceDestroyed(OH_NativeXComponent* c, void*) {
    auto* state = Enter(c);
    if (!state) return;
    state->touchId = -1;
    attached = false;
    gc_detach();
}
void Touch(OH_NativeXComponent* c, void* window) {
    auto* state = Enter(c);
    if (!state) return;
    auto& touchId = state->touchId;
    OH_NativeXComponent_TouchEvent e{};
    if (OH_NativeXComponent_GetTouchEvent(c, window, &e) != OH_NATIVEXCOMPONENT_RESULT_SUCCESS) return;
    uint32_t index = 0;
    while (index < e.numPoints && index < OH_NATIVE_XCOMPONENT_MAX_TOUCH_POINTS_NUMBER && e.touchPoints[index].id != e.id) ++index;
    OH_NativeXComponent_TouchPointToolType tool = OH_NATIVEXCOMPONENT_TOOL_TYPE_FINGER;
    if (index < e.numPoints && index < OH_NATIVE_XCOMPONENT_MAX_TOUCH_POINTS_NUMBER)
        OH_NativeXComponent_GetTouchPointToolType(c, index, &tool);
    // v1: finger touches only; stylus hover/draw is a later addition.
    if (tool != OH_NATIVEXCOMPONENT_TOOL_TYPE_FINGER) return;
    if (e.type == OH_NATIVEXCOMPONENT_CANCEL) {
        touchId = -1;
        Json("{\"kind\":\"blur\"}");
        return;
    }
    std::string points;
    for (uint32_t i = 0; i < e.numPoints && i < OH_NATIVE_XCOMPONENT_MAX_TOUCH_POINTS_NUMBER; ++i) {
        const auto& point = e.touchPoints[i];
        OH_NativeXComponent_TouchPointToolType pointTool = OH_NATIVEXCOMPONENT_TOOL_TYPE_FINGER;
        OH_NativeXComponent_GetTouchPointToolType(c, i, &pointTool);
        if (pointTool != OH_NATIVEXCOMPONENT_TOOL_TYPE_FINGER ||
            point.type == OH_NATIVEXCOMPONENT_UP || point.type == OH_NATIVEXCOMPONENT_CANCEL ||
            (point.id == e.id && e.type == OH_NATIVEXCOMPONENT_UP) ||
            !std::isfinite(point.x) || !std::isfinite(point.y)) continue;
        if (!points.empty()) points += ",";
        points += "{\"id\":" + std::to_string(static_cast<uint32_t>(point.id)) + ",\"x\":" + std::to_string(point.x) + ",\"y\":" + std::to_string(point.y) + "}";
    }
    Json("{\"kind\":\"touch\",\"points\":[" + points + "]}");
}
void Mouse(OH_NativeXComponent* c, void* window) {
    auto* state = Enter(c);
    if (!state) return;
    OH_NativeXComponent_MouseEvent e{};
    if (OH_NativeXComponent_GetMouseEvent(c, window, &e) != OH_NATIVEXCOMPONENT_RESULT_SUCCESS) return;
    if (e.action == OH_NATIVEXCOMPONENT_MOUSE_CANCEL) { Json("{\"kind\":\"pointer\",\"action\":3}"); return; }
    if (e.action != OH_NATIVEXCOMPONENT_MOUSE_PRESS && e.action != OH_NATIVEXCOMPONENT_MOUSE_RELEASE &&
        e.action != OH_NATIVEXCOMPONENT_MOUSE_MOVE) return;
    int action = e.action == OH_NATIVEXCOMPONENT_MOUSE_PRESS ? 0 : e.action == OH_NATIVEXCOMPONENT_MOUSE_RELEASE ? 1 : 2;
    int button = e.button == OH_NATIVEXCOMPONENT_RIGHT_BUTTON ? 1 : e.button == OH_NATIVEXCOMPONENT_MIDDLE_BUTTON ? 2 : 0;
    Json("{\"kind\":\"pointer\",\"action\":" + std::to_string(action) + ",\"button\":" + std::to_string(button) + ",\"x\":" + std::to_string(e.x) + ",\"y\":" + std::to_string(e.y) + "}");
}
bool Key(OH_NativeXComponent* c, void*) {
    if (!Enter(c)) return false;
    OH_NativeXComponent_KeyEvent* event = nullptr;
    OH_NativeXComponent_KeyAction action = OH_NATIVEXCOMPONENT_KEY_ACTION_UNKNOWN;
    OH_NativeXComponent_KeyCode code = static_cast<OH_NativeXComponent_KeyCode>(-1);
    if (OH_NativeXComponent_GetKeyEvent(c, &event) != 0 || !event ||
        OH_NativeXComponent_GetKeyEventAction(event, &action) != 0 ||
        OH_NativeXComponent_GetKeyEventCode(event, &code) != 0) return false;
    uint64_t modifiers = 0; bool caps = false;
    OH_NativeXComponent_GetKeyEventModifierKeyStates(event, &modifiers);
    OH_NativeXComponent_GetKeyEventCapsLockState(event, &caps);
    bool ctrl = modifiers & 1, shift = modifiers & 2, alt = modifiers & 4;
    int key = static_cast<int>(code);
    std::string text;
    if (!ctrl && !alt && action == OH_NATIVEXCOMPONENT_KEY_ACTION_DOWN) {
        if (key >= 2017 && key <= 2042) text.push_back(static_cast<char>((shift != caps ? 'A' : 'a') + key - 2017));
        else if (key >= 2000 && key <= 2009) text.push_back(shift ? ") !@#$%^&*("[key - 2000] : static_cast<char>('0' + key - 2000));
        else if (key == 2050) text = " ";
        else {
            const int codes[] = {2043, 2044, 2056, 2057, 2058, 2059, 2060, 2061, 2062, 2063, 2064};
            const char* plain = ",.`-=[]\\;'/";
            const char* shifted = "<>~_+{}|:\"?";
            for (size_t i = 0; i < sizeof(codes) / sizeof(codes[0]); ++i) if (key == codes[i]) text.push_back(shift ? shifted[i] : plain[i]);
        }
    }
    if (text == "\\") text = "\\\\";
    else if (text == "\"") text = "\\\"";
    Json("{\"kind\":\"key\",\"action\":" + std::to_string(static_cast<int>(action)) +
         ",\"code\":" + std::to_string(key) + ",\"ctrl\":" + (ctrl ? "true" : "false") +
         ",\"shift\":" + (shift ? "true" : "false") + ",\"alt\":" + (alt ? "true" : "false") +
         ",\"text\":\"" + text + "\"}");
    return true;
}
void Axis(OH_NativeXComponent* c, ArkUI_UIInputEvent* event, ArkUI_UIInputEvent_Type type) {
    auto* state = Enter(c);
    if (!state) return;
    if (!event || type != ARKUI_UIINPUTEVENT_TYPE_AXIS) return;
    double x = -OH_ArkUI_AxisEvent_GetHorizontalAxisValue(event);
    double y = -OH_ArkUI_AxisEvent_GetVerticalAxisValue(event);
    double zoom = OH_ArkUI_AxisEvent_GetPinchAxisScaleValue(event);
    float px = OH_ArkUI_PointerEvent_GetX(event), py = OH_ArkUI_PointerEvent_GetY(event);
    if (!std::isfinite(x) || !std::isfinite(y) || !std::isfinite(px) || !std::isfinite(py)) return;
    if (!std::isfinite(zoom) || zoom < 0) zoom = 0;
    Json("{\"kind\":\"axis\",\"action\":" + std::to_string(OH_ArkUI_AxisEvent_GetAxisAction(event)) +
         ",\"x\":" + std::to_string(px) + ",\"y\":" + std::to_string(py) + ",\"dx\":" + std::to_string(x) +
         ",\"dy\":" + std::to_string(y) + ",\"zoom\":" + std::to_string(zoom) + "}");
}
void Blur(OH_NativeXComponent* c, void*) {
    auto* state = Enter(c);
    if (!state) return;
    state->touchId = -1;
    Json("{\"kind\":\"blur\"}");
}
void Hover(OH_NativeXComponent* c, bool isHover) {
    auto* state = Enter(c);
    if (!state) return;
    if (!isHover && state->touchId == -1) Json("{\"kind\":\"hover_leave\"}");
}
OH_NativeXComponent_Callback callbacks{SurfaceCreated, SurfaceChanged, SurfaceDestroyed, Touch};
OH_NativeXComponent_MouseEvent_Callback mouseCallbacks{Mouse, Hover};

// Register the shared callback set for one XComponent (main window of v1, key 0).
bool RegisterComponent(OH_NativeXComponent* c, uint32_t explicit_key) {
    if (!c) return false;
    char id[OH_XCOMPONENT_ID_LEN_MAX + 1] = {0};
    uint64_t size = OH_XCOMPONENT_ID_LEN_MAX + 1;
    if (OH_NativeXComponent_GetXComponentId(c, id, &size) != OH_NATIVEXCOMPONENT_RESULT_SUCCESS) return false;
    uint32_t key = explicit_key;
    {
        std::lock_guard<std::mutex> lock(winMutex);
        auto it = windows.find(c);
        if (it == windows.end() || it->second->key != key) {
            auto state = std::make_shared<WinState>();
            state->key = key;
            windows[c] = state;
        } else {
            it->second->key = key;
        }
    }
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft",
        "Registering XComponent callbacks id=%{public}s window=%{public}u explicit=%{public}u", id, key, explicit_key);
    OH_NativeXComponent_RegisterCallback(c, &callbacks);
    OH_NativeXComponent_RegisterMouseEventCallback(c, &mouseCallbacks);
    OH_NativeXComponent_RegisterKeyEventCallbackWithResult(c, Key);
    OH_NativeXComponent_RegisterBlurEventCallback(c, Blur);
    OH_NativeXComponent_RegisterUIInputEventCallback(c, Axis, ARKUI_UIINPUTEVENT_TYPE_AXIS);
    return true;
}
napi_value InitHost(napi_env env, napi_callback_info info) {
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft", "Initializing Rust host");
    size_t n = 2; napi_value args[2]; napi_get_cb_info(env, info, &n, args, nullptr, nullptr);
    if (n != 2) return Undefined(env);
    auto config = String(env, args[0]);
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft", "Config %{public}s", config.c_str());
    {
        std::lock_guard<std::mutex> lock(notifyMutex);
        if (notifyFn) napi_release_threadsafe_function(notifyFn, napi_tsfn_abort);
        napi_value name; napi_create_string_utf8(env, "GridCraft platform", NAPI_AUTO_LENGTH, &name);
        auto status = napi_create_threadsafe_function(env, args[1], nullptr, name, 0, 1, nullptr, nullptr, nullptr, CallJS, &notifyFn);
        OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft", "TSFN status %{public}d", static_cast<int>(status));
        if (status != napi_ok) return Undefined(env);
        napi_unref_threadsafe_function(env, notifyFn);
    }
    if (!vsync) vsync = OH_NativeVSync_Create("GridCraft", 10);
    bool started = gc_init(reinterpret_cast<const uint8_t*>(config.data()), config.size(), NotifyRust);
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft", "Rust started %{public}d", started);
    if (!started) {
        const std::string json = "{\"kind\":\"error\",\"text\":\"Native configuration failed\"}";
        NotifyRust(0, reinterpret_cast<const uint8_t*>(json.data()), json.size(), nullptr, 0);
    }
    return Undefined(env);
}
napi_value Input(napi_env env, napi_callback_info info) {
    size_t n = 1; napi_value arg; napi_get_cb_info(env, info, &n, &arg, nullptr, nullptr);
    cur = nullptr;
    if (n) { auto s = String(env, arg); Json(s); }
    return Undefined(env);
}
napi_value Command(napi_env env, napi_callback_info info) {
    size_t n = 1; napi_value arg; napi_get_cb_info(env, info, &n, &arg, nullptr, nullptr);
    if (n) { auto s = String(env, arg); gc_command(reinterpret_cast<const uint8_t*>(s.data()), s.size()); }
    return Undefined(env);
}
napi_value Reply(napi_env env, napi_callback_info info) {
    size_t n = 4; napi_value args[4]; napi_get_cb_info(env, info, &n, args, nullptr, nullptr);
    if (n != 4) return Undefined(env);
    double id = 0; bool ok = false; void* data = nullptr; size_t size = 0;
    napi_get_value_double(env, args[0], &id); napi_get_value_bool(env, args[1], &ok);
    auto text = String(env, args[2]); napi_get_arraybuffer_info(env, args[3], &data, &size);
    gc_reply(static_cast<uint64_t>(id), ok, reinterpret_cast<const uint8_t*>(text.data()), text.size(), static_cast<uint8_t*>(data), size);
    return Undefined(env);
}
napi_value Open(napi_env env, napi_callback_info info) {
    size_t n = 2; napi_value args[2]; napi_get_cb_info(env, info, &n, args, nullptr, nullptr);
    if (n != 2) return Undefined(env);
    auto name = String(env, args[0]); void* data = nullptr; size_t size = 0; napi_get_arraybuffer_info(env, args[1], &data, &size);
    gc_open(reinterpret_cast<const uint8_t*>(name.data()), name.size(), static_cast<uint8_t*>(data), size);
    return Undefined(env);
}
napi_value DropFile(napi_env env, napi_callback_info info) {
    size_t n = 4; napi_value args[4]; napi_get_cb_info(env, info, &n, args, nullptr, nullptr);
    if (n != 4) return Undefined(env);
    int32_t fd = -1; double x = 0, y = 0;
    if (napi_get_value_int32(env, args[1], &fd) != napi_ok || napi_get_value_double(env, args[2], &x) != napi_ok ||
        napi_get_value_double(env, args[3], &y) != napi_ok) return Undefined(env);
    auto name = String(env, args[0]);
    gc_drop_file(reinterpret_cast<const uint8_t*>(name.data()), name.size(), fd, static_cast<float>(x), static_cast<float>(y));
    return Undefined(env);
}
napi_value Drop(napi_env env, napi_callback_info info) {
    size_t n = 4; napi_value args[4]; napi_get_cb_info(env, info, &n, args, nullptr, nullptr);
    if (n != 4) return Undefined(env);
    auto name = String(env, args[0]); void* data = nullptr; size_t size = 0; double x = 0, y = 0;
    if (napi_get_arraybuffer_info(env, args[1], &data, &size) != napi_ok ||
        napi_get_value_double(env, args[2], &x) != napi_ok || napi_get_value_double(env, args[3], &y) != napi_ok) return Undefined(env);
    gc_drop(reinterpret_cast<const uint8_t*>(name.data()), name.size(), static_cast<uint8_t*>(data), size, static_cast<float>(x), static_cast<float>(y));
    return Undefined(env);
}
napi_value Active(napi_env env, napi_callback_info info) {
    size_t n = 1; napi_value arg; bool active = false; napi_get_cb_info(env, info, &n, &arg, nullptr, nullptr);
    if (n) napi_get_value_bool(env, arg, &active); gc_active(active); return Undefined(env);
}
napi_value Close(napi_env env, napi_callback_info) { gc_close(); return Undefined(env); }
void Cleanup(void*) {
    attached = false; gc_active(false); gc_detach();
    std::lock_guard<std::mutex> lock(notifyMutex);
    if (notifyFn) { napi_release_threadsafe_function(notifyFn, napi_tsfn_abort); notifyFn = nullptr; }
}
napi_value Init(napi_env env, napi_value exports) {
    OH_LOG_Print(LOG_APP, LOG_INFO, 0xD001, "GridCraft", "Loading NAPI module");
    const napi_property_descriptor props[] = {
        {"init", nullptr, InitHost, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"input", nullptr, Input, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"reply", nullptr, Reply, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"open", nullptr, Open, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"dropFile", nullptr, DropFile, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"drop", nullptr, Drop, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"active", nullptr, Active, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"close", nullptr, Close, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"command", nullptr, Command, nullptr, nullptr, nullptr, napi_default, nullptr}
    };
    napi_define_properties(env, exports, sizeof(props) / sizeof(props[0]), props);
    napi_value value; OH_NativeXComponent* c = nullptr;
    if (napi_get_named_property(env, exports, OH_NATIVE_XCOMPONENT_OBJ, &value) == napi_ok &&
        napi_unwrap(env, value, reinterpret_cast<void**>(&c)) == napi_ok && c) {
        RegisterComponent(c, 0);
    }
    napi_add_env_cleanup_hook(env, Cleanup, nullptr);
    return exports;
}
napi_module module{1, 0, nullptr, Init, "gridcraft", nullptr, {0}};
}

extern "C" void gc_request_vsync() {
    if (vsync && attached && !framePending.exchange(true)) {
        if (OH_NativeVSync_RequestFrame(vsync, Vsync, nullptr) != 0) framePending = false;
    }
}

extern "C" __attribute__((constructor)) void RegisterGridCraft() { napi_module_register(&module); }
