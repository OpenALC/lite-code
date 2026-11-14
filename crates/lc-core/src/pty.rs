//! PTY 终端会话层（P5 issue #63：docs/PLAN.md「终端与 Agent 会话必须活在 Rust 侧」）。
//!
//! 本地 shell 与 SSH 统一为一个抽象：SSH 不引库，直接在 pty 里启动系统 `ssh`
//! 二进制（`ssh [-p port] [-l user] host`），主机密钥确认与密码交互全部走 ssh
//! 自身的终端提示——pty 本来就是交互终端。
//!
//! 职责边界：
//! - [`SessionManager`] 是纯 Rust 状态机，事件经注入的回调上抛（Tauri 命令层
//!   映射为 `pty:data` / `pty:state` / `pty:exit`）；真实 IO 由 [`spawn_real`]
//!   提供，单测注入内存 fake，不依赖平台 pty；
//! - 会话活在 Rust 侧：注册表是 app 级状态，窗口销毁/重建不影响（不绑定
//!   WebviewWindow）；`list` 供重开窗口恢复标签；
//! - 滚动缓冲：每会话保留最近 [`MAX_SCROLLBACK`] 输出（环形，[`RingBuf`]），
//!   重连成功后作为一条 `Data` 事件重放，供 `pty_replay` 恢复屏幕；
//! - 自动重连：子进程**非用户关闭**的退出 → `Reconnecting`，同参数重建一次
//!   并重放缓冲；再次退出 → `Closed`。用户 `close` 的会话不再重连；
//! - 退出回收：[`SessionManager::kill_all`] 在 Drop 与应用退出钩子里都调用
//!   （docs/PLAN.md R5：不留孤儿）。
//!
//! 线程模型：每会话一条读线程（阻塞读 → 缓冲 → 事件）；写/resize/close 走
//! [`PtyControl`]（含 portable-pty 的 master writer），命令线程直接持有，
//! 不与阻塞读争锁。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// 每会话滚动缓冲上限（验收：≤256 KiB）。
pub const MAX_SCROLLBACK: usize = 256 * 1024;

/// 会话类型与启动参数（重连按同参数重建）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    /// 本地 shell：pwsh 7 优先（fallback powershell.exe/cmd，Unix $SHELL/bash）。
    Local,
    /// 系统 ssh 二进制；`port` 默认 22，`user` 可空。
    Ssh {
        host: String,
        port: u16,
        user: Option<String>,
    },
}

impl Spec {
    /// `pty_list` 的 `kind` 字段。
    pub fn kind(&self) -> &'static str {
        match self {
            Spec::Local => "local",
            Spec::Ssh { .. } => "ssh",
        }
    }
}

/// 会话状态（验收契约：connected|reconnecting|closed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Connected,
    Reconnecting,
    Closed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Connected => "connected",
            State::Reconnecting => "reconnecting",
            State::Closed => "closed",
        }
    }
}

/// 上抛事件（Tauri 命令层映射为 `pty:*` 事件；`exit.code` 为退出码，
/// 信号终止/无法取得时为 -1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    Data { session_id: String, data: Vec<u8> },
    State { session_id: String, state: State },
    Exit { session_id: String, code: i32 },
}

/// 环形滚动缓冲：保留最近 `cap` 字节（[`MAX_SCROLLBACK`]）。
#[derive(Debug)]
pub struct RingBuf {
    data: Vec<u8>,
    cap: usize,
}

impl RingBuf {
    pub fn new(cap: usize) -> Self {
        Self {
            data: Vec::new(),
            cap,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= self.cap {
            self.data.clear();
            self.data
                .extend_from_slice(&chunk[chunk.len() - self.cap..]);
            return;
        }
        let total = self.data.len() + chunk.len();
        if total > self.cap {
            self.data.drain(..total - self.cap);
        }
        self.data.extend_from_slice(chunk);
    }

    /// 全量缓冲快照（`pty_replay` 返回值 / 重连重放）。
    pub fn snapshot(&self) -> Vec<u8> {
        self.data.clone()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// 命令线程操作的 IO 半区：写、resize、终止（close / kill_all 共用）。
/// `kill` 必须终止子进程并关闭 pty（master）。
pub trait PtyControl: Send {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()>;
    fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()>;
    fn kill(&mut self);
}

/// 会话线程持有的 IO 半区：阻塞读 + 子进程退出探测。
pub trait PtyReader: Send {
    /// 阻塞读一段输出；EOF/关闭返回 0 长度。
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    /// 子进程是否已退出（`Some` 后不再变化）。用 portable-pty 的
    /// `ExitStatus`（可从 `std` 转换，测试可直接构造）。
    fn try_wait(&mut self) -> Option<portable_pty::ExitStatus>;
}

/// 一次 spawn 的产物：两半区各归其主。
pub struct PtyHandles {
    pub control: Box<dyn PtyControl>,
    pub reader: Box<dyn PtyReader>,
}

/// pty 工厂：按 spec + 尺寸启动一个会话进程。真实实现见 [`spawn_real`]。
pub type PtyFactory = dyn FnMut(&Spec, u16, u16) -> std::io::Result<PtyHandles> + Send;

struct SessionInner {
    spec: Spec,
    title: Mutex<String>,
    scrollback: Mutex<RingBuf>,
    state: Mutex<State>,
    size: Mutex<(u16, u16)>,
    user_closed: AtomicBool,
    control: Mutex<Option<Box<dyn PtyControl>>>,
}

/// 会话的线程安全句柄（命令层与会话线程共享）。
#[derive(Clone)]
pub struct Session {
    id: String,
    inner: Arc<SessionInner>,
}

impl Session {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn spec(&self) -> &Spec {
        &self.inner.spec
    }

    pub fn state(&self) -> State {
        *self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn title(&self) -> String {
        self.inner
            .title
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn scrollback(&self) -> Vec<u8> {
        self.inner
            .scrollback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot()
    }

    /// 用户主动关闭（`pty_close`）：幂等；标记后 kill 子进程，会话线程
    /// 读到 EOF 后落终态，不再重连。
    pub fn close(&self) {
        self.inner.user_closed.store(true, Ordering::SeqCst);
        self.mark_state(State::Closed);
        self.take_control_and_kill();
    }

    /// 写入（base64 解码后的字节；会话已关闭时静默丢弃）。
    pub fn write(&self, data: &[u8]) {
        let mut guard = self
            .inner
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(control) = guard.as_mut() {
            let _ = control.write(data);
        }
    }

    /// resize：记录新尺寸（重连按此重建）并转发给 pty。
    pub fn resize(&self, cols: u16, rows: u16) {
        *self
            .inner
            .size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = (cols, rows);
        let mut guard = self
            .inner
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(control) = guard.as_mut() {
            let _ = control.resize(cols, rows);
        }
    }

    fn size(&self) -> (u16, u16) {
        *self
            .inner
            .size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn is_user_closed(&self) -> bool {
        self.inner.user_closed.load(Ordering::SeqCst)
    }

    fn mark_state(&self, state: State) {
        *self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = state;
    }

    fn push_output(&self, chunk: &[u8]) {
        self.inner
            .scrollback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(chunk);
    }

    fn set_control(&self, control: Box<dyn PtyControl>) {
        *self
            .inner
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(control);
    }

    fn take_control_and_kill(&self) {
        if let Some(mut control) = self
            .inner
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            control.kill();
        }
    }
}

fn default_title(spec: &Spec) -> String {
    match spec {
        Spec::Local => {
            if cfg!(windows) {
                "PowerShell".into()
            } else {
                "bash".into()
            }
        }
        Spec::Ssh { host, .. } => format!("ssh://{host}"),
    }
}

struct ManagerInner {
    sessions: Mutex<HashMap<String, Session>>,
    counter: AtomicU64,
    on_event: Box<dyn Fn(PtyEvent) + Send + Sync>,
    factory: Mutex<Box<PtyFactory>>,
}

/// 会话管理器：app 级全局（Tauri State 包一层）。Drop 时回收全部子进程。
pub struct SessionManager {
    inner: Arc<ManagerInner>,
    exit: Arc<AtomicBool>,
}

impl SessionManager {
    pub fn new(factory: Box<PtyFactory>, on_event: Box<dyn Fn(PtyEvent) + Send + Sync>) -> Self {
        Self {
            inner: Arc::new(ManagerInner {
                sessions: Mutex::new(HashMap::new()),
                counter: AtomicU64::new(0),
                on_event,
                factory: Mutex::new(factory),
            }),
            exit: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 打开会话：注册 → 首次 spawn → 起会话线程。spawn 失败直接返回错误
    /// （会话不入表，命令层拿到可操作的错误）。
    pub fn open(&self, spec: Spec, cols: u16, rows: u16) -> Result<Session, PtyError> {
        let handles =
            (self
                .inner
                .factory
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()))(&spec, cols, rows)
            .map_err(|e| PtyError::Spawn(e.to_string()))?;
        let id = self.allocate_id();
        let session = Session {
            id: id.clone(),
            inner: Arc::new(SessionInner {
                title: Mutex::new(default_title(&spec)),
                spec,
                scrollback: Mutex::new(RingBuf::new(MAX_SCROLLBACK)),
                state: Mutex::new(State::Connected),
                size: Mutex::new((cols, rows)),
                user_closed: AtomicBool::new(false),
                control: Mutex::new(Some(handles.control)),
            }),
        };
        self.lock().insert(id, session.clone());
        self.spawn_reader(session.clone(), handles.reader);
        Ok(session)
    }

    fn allocate_id(&self) -> String {
        let n = self.inner.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0);
        format!("pty-{millis}-{n}")
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        self.lock().get(id).cloned()
    }

    /// `pty_list`：重开窗口恢复标签用（含 closed 会话，缓冲保留供 replay）。
    pub fn list(&self) -> Vec<Session> {
        let mut sessions: Vec<Session> = self.lock().values().cloned().collect();
        sessions.sort_by(|a, b| a.id().cmp(b.id()));
        sessions
    }

    /// 用户关闭（幂等）：未知会话报错，已 closed 的会话原样返回 Ok。
    pub fn close(&self, id: &str) -> Result<(), PtyError> {
        let session = self
            .get(id)
            .ok_or_else(|| PtyError::UnknownSession(id.to_string()))?;
        session.close();
        Ok(())
    }

    /// 应用退出 / Drop：置退出位并 kill 全部子进程（验收第 5 条）。
    /// 幂等：重复调用安全。
    pub fn kill_all(&self) {
        self.exit.store(true, Ordering::SeqCst);
        for session in self.list() {
            session.mark_state(State::Closed);
            session.take_control_and_kill();
        }
    }

    pub(crate) fn spawn_reader(&self, session: Session, mut reader: Box<dyn PtyReader>) {
        let inner = Arc::clone(&self.inner);
        let exit = Arc::clone(&self.exit);
        let spawned = std::thread::Builder::new()
            .name(format!("pty-{}", session.id))
            .spawn({
                let session = session.clone();
                move || run_session(&inner, &exit, &session, &mut reader)
            });
        if spawned.is_err() {
            // 起线程失败：不留活进程，落终态。
            session.close();
        }
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

/// 会话线程主体：读输出 → 事件；退出按重连语义收尾。
fn run_session(
    inner: &Arc<ManagerInner>,
    exit: &AtomicBool,
    session: &Session,
    reader: &mut Box<dyn PtyReader>,
) {
    let mut can_reconnect = true;
    loop {
        let ended = pump_output(inner, exit, session, reader.as_mut());
        if matches!(ended, PumpEnd::Shutdown) {
            // 应用退出：不发声，静默回收。
            return;
        }
        let code = match ended {
            PumpEnd::Exit(code) => code,
            PumpEnd::Shutdown => unreachable!(),
        };
        emit(
            inner,
            PtyEvent::Exit {
                session_id: session.id().to_string(),
                code,
            },
        );
        if session.is_user_closed() {
            emit_state(inner, session, State::Closed);
            return;
        }
        if !can_reconnect {
            emit_state(inner, session, State::Closed);
            return;
        }
        can_reconnect = false;
        emit_state(inner, session, State::Reconnecting);
        let (cols, rows) = session.size();
        let handles = match (inner
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))(
            session.spec(), cols, rows
        ) {
            Ok(handles) => handles,
            Err(_) => {
                emit_state(inner, session, State::Closed);
                return;
            }
        };
        session.set_control(handles.control);
        emit_state(inner, session, State::Connected);
        // 重放滚动缓冲：web 端无需主动 pty_replay 即可恢复屏幕。
        let snapshot = session.scrollback();
        if !snapshot.is_empty() {
            emit(
                inner,
                PtyEvent::Data {
                    session_id: session.id().to_string(),
                    data: snapshot,
                },
            );
        }
        *reader = handles.reader;
    }
}

enum PumpEnd {
    /// 子进程退出，携带退出码（信号终止/未知为 -1）。
    Exit(i32),
    /// 应用退出信号触发，静默回收。
    Shutdown,
}

/// 读循环：读到 EOF/错误或探测到子进程退出为止。返回结束原因。
fn pump_output(
    inner: &Arc<ManagerInner>,
    exit: &AtomicBool,
    session: &Session,
    reader: &mut dyn PtyReader,
) -> PumpEnd {
    let mut buf = [0u8; 8192];
    loop {
        if exit.load(Ordering::SeqCst) {
            return PumpEnd::Shutdown;
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                session.push_output(&buf[..n]);
                emit(
                    inner,
                    PtyEvent::Data {
                        session_id: session.id().to_string(),
                        data: buf[..n].to_vec(),
                    },
                );
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        if let Some(status) = reader.try_wait() {
            return PumpEnd::Exit(exit_code_of(status));
        }
    }
    let code = reader.try_wait().map(exit_code_of).unwrap_or(-1);
    PumpEnd::Exit(code)
}

fn exit_code_of(status: portable_pty::ExitStatus) -> i32 {
    if status.signal().is_some() {
        return -1;
    }
    status.exit_code() as i32
}

fn emit(inner: &Arc<ManagerInner>, event: PtyEvent) {
    (inner.on_event)(event);
}

fn emit_state(inner: &Arc<ManagerInner>, session: &Session, state: State) {
    // 状态字段与事件同步落库：`pty_list` / 命令层读的是会话字段，
    // 不能只发事件不改状态。
    session.mark_state(state);
    emit(
        inner,
        PtyEvent::State {
            session_id: session.id().to_string(),
            state,
        },
    );
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PtyError {
    #[error("会话不存在：{0}")]
    UnknownSession(String),
    #[error("无法启动终端进程：{0}")]
    Spawn(String),
}

// ==== 真实实现（portable-pty + 系统 ssh） ====

mod real;

pub use real::{spawn_real, ssh_command};

#[cfg(test)]
mod tests;

/// 测试夹具：可编程 fake pty（生产二进制不带）。
#[cfg(test)]
pub(crate) mod tests_support {
    use std::io;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// fake 读循环轮询的单步间隔。
    pub(crate) const POLL_STEP: Duration = Duration::from_millis(5);
    /// fake 读循环的兜底超时（防止测试悬挂）。
    pub(crate) const POLL_LIMIT: Duration = Duration::from_secs(5);

    /// 可编程 fake pty：control 半区记录写入/kill/resize，reader 半区按
    /// 脚本出队输出；`exit` 模拟子进程退出（kill 会置 0）。
    #[derive(Clone)]
    pub(crate) struct FakePty {
        pub(crate) killed: Arc<AtomicBool>,
        pub(crate) written: Arc<Mutex<Vec<u8>>>,
        pub(crate) output: Arc<Mutex<Vec<Vec<u8>>>>,
        pub(crate) exit: Arc<Mutex<Option<i32>>>,
        pub(crate) sizes: Arc<Mutex<Vec<(u16, u16)>>>,
    }

    impl FakePty {
        pub(crate) fn new() -> Self {
            Self {
                killed: Arc::new(AtomicBool::new(false)),
                written: Arc::new(Mutex::new(Vec::new())),
                output: Arc::new(Mutex::new(Vec::new())),
                exit: Arc::new(Mutex::new(None)),
                sizes: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    struct FakeControl {
        pty: FakePty,
    }

    impl super::PtyControl for FakeControl {
        fn write(&mut self, data: &[u8]) -> io::Result<()> {
            self.pty.written.lock().unwrap().extend_from_slice(data);
            Ok(())
        }

        fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
            self.pty.sizes.lock().unwrap().push((cols, rows));
            Ok(())
        }

        fn kill(&mut self) {
            self.pty
                .killed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            *self.pty.exit.lock().unwrap() = Some(0);
        }
    }

    struct FakeReader {
        pty: FakePty,
    }

    impl super::PtyReader for FakeReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let deadline = std::time::Instant::now() + POLL_LIMIT;
            loop {
                if self.pty.exit.lock().unwrap().is_some() {
                    // 已退出：先排空剩余输出，再报 EOF
                    let mut queue = self.pty.output.lock().unwrap();
                    if let Some(chunk) = queue.first().cloned() {
                        let n = chunk.len().min(buf.len());
                        buf[..n].copy_from_slice(&chunk[..n]);
                        if n == chunk.len() {
                            queue.remove(0);
                        } else {
                            queue[0] = chunk[n..].to_vec();
                        }
                        return Ok(n);
                    }
                    return Ok(0);
                }
                let chunk = {
                    let mut queue = self.pty.output.lock().unwrap();
                    if queue.is_empty() {
                        None
                    } else {
                        Some(queue.remove(0))
                    }
                };
                if let Some(chunk) = chunk {
                    let n = chunk.len().min(buf.len());
                    buf[..n].copy_from_slice(&chunk[..n]);
                    if n < chunk.len() {
                        self.pty
                            .output
                            .lock()
                            .unwrap()
                            .insert(0, chunk[n..].to_vec());
                    }
                    return Ok(n);
                }
                if std::time::Instant::now() >= deadline {
                    return Ok(0);
                }
                std::thread::sleep(POLL_STEP);
            }
        }

        fn try_wait(&mut self) -> Option<portable_pty::ExitStatus> {
            // fake 走 EOF 通道：退出时把 exit 字段模拟成 portable-pty 的状态。
            self.pty
                .exit
                .lock()
                .unwrap()
                .map(|code| portable_pty::ExitStatus::with_exit_code(code.max(0) as u32))
        }
    }

    impl FakePty {
        pub(crate) fn handles(&self) -> super::PtyHandles {
            super::PtyHandles {
                control: Box::new(FakeControl { pty: self.clone() }),
                reader: Box::new(FakeReader { pty: self.clone() }),
            }
        }
    }
}
