//! 鸿蒙 N-API 桥：Rust 侧只通过一个通知回调上报状态、通过请求/应答通道向 ArkTS 要文件/服务。
//! 平台只等待 Rust worker 线程，绝不阻塞 ArkTS 主线程。
use std::collections::HashMap;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};

/// ArkTS 侧的通知回调：`(id, kind, text, data)`。C++ 边界会在返回前拷贝两块借用的切片。
pub type Notify = extern "C" fn(u64, *const u8, usize, *const u8, usize);
static NOTIFY: OnceLock<Notify> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static CANCELLED: AtomicBool = AtomicBool::new(false);
static PENDING: OnceLock<Mutex<HashMap<u64, mpsc::Sender<Reply>>>> = OnceLock::new();

/// 一次请求的结果：`(文件名, 字节)` 或错误串。
pub type Reply = Result<(String, Vec<u8>), String>;

fn pending() -> &'static Mutex<HashMap<u64, mpsc::Sender<Reply>>> {
    PENDING.get_or_init(Default::default)
}

pub fn init(notify: Notify) {
    let _ = NOTIFY.set(notify);
}

/// 向 ArkTS 上报一条状态/事件。kind 形如 "stage"/"error"/"diagnostic"/"ready" 等。
pub fn notify(id: u64, kind: &str, text: &str, data: &[u8]) {
    if let Some(f) = NOTIFY.get() {
        let message = serde_json::json!({ "kind": kind, "text": text }).to_string();
        // C++ 回调会在返回前拷贝这两块借用的切片。
        f(
            id,
            message.as_ptr(),
            message.len(),
            data.as_ptr(),
            data.len(),
        );
    }
}

/// 同步请求：登记应答通道 → 通知平台 → 阻塞等待 ArkTS 回包。返回文件名与字节。
pub fn request(kind: &str, text: &str, data: &[u8]) -> Reply {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    {
        let mut map = pending().lock().unwrap_or_else(|p| p.into_inner());
        if CANCELLED.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        map.insert(id, tx);
    }
    notify(id, kind, text, data);
    rx.recv()
        .unwrap_or_else(|_| Err("platform service disconnected".into()))
}

/// ArkTS 回包：按 id 找到等待者并投递。
pub fn reply(id: u64, result: Reply) {
    if let Some(tx) = pending()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&id)
    {
        let _ = tx.send(result);
    }
}

/// 生命周期中止时唤醒所有等待者，避免 worker 永久卡在已关闭的系统选择器上。
pub fn cancel() {
    let mut map = pending().lock().unwrap_or_else(|p| p.into_inner());
    CANCELLED.store(true, Ordering::Release);
    for (_, tx) in map.drain() {
        let _ = tx.send(Err("cancelled".into()));
    }
}

pub fn resume() {
    CANCELLED.store(false, Ordering::Release);
}
