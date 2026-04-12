在当前的项目中有两个概念：Session 和 Thread。

目前存在一个问题，即 Thread 本身持有需要落盘的状态，并将 Thread 作为核心状态。在向其他参数传递的过程中，需要对 Thread 进行读写。如果你持有整个 Thread 并让其显式管理所有状态，会导致以下问题：

1. 锁粒度过大：Thread 持有锁的时间太长，导致其他地方无法查看或访问。
2. 实现不优雅：为了实现 Thread 每发送一个消息就能落盘，在 Rust 中，Session 与 Thread 之间的相互引用处理起来很不优雅。Session 管理 Thread，而 Thread 的数据又要写回 Session，这种引用关系过于复杂。

因此，我打算将其重构成如下形式：
1. 职责分离：Session 依然负责管理 Session 逻辑，并由其派生出 Thread。
2. 句柄化：向外派生的其实是 Thread 的一个句柄（Handle），这是一种能够找到 Thread 的方式。
3. 状态管理下沉：Thread 最终的状态管理被抽离到一个 Storage 层。
4. 优化并发：每次通过读写锁从 Storage 层获取状态，得到对应数据后立即释放锁。

通过这种方式，实现了 Thread 与其自身状态的解耦。外部持有的只是句柄，而实际的读写则通过读写锁去操作 Storage 层。


下面是一个demo 示例这种refact形式
```rust
use std::{
    collections::HashMap,
    fs,
    io,
    path::PathBuf,
    sync::{Arc, LockResult, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use uuid::Uuid;

#[derive(Clone)]
struct Session(Arc<SessionInner>);

struct SessionInner {
    threads: Mutex<HashMap<Uuid, ThreadState>>,
    storage: Storage,
}

#[derive(Clone)]
struct Storage(Arc<StorageInner>);

struct StorageInner {
    path: PathBuf,
    data: Mutex<HashMap<String, Vec<String>>>,
}

struct Thread {
    id: Uuid,
    session: Session,
    state: ThreadState,
}

#[derive(Clone)]
struct ThreadState(Arc<RwLock<ThreadStateInner>>);

struct ThreadStateInner {
    messages: Vec<String>,
}

impl Session {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self(Arc::new(SessionInner {
            threads: Mutex::new(HashMap::new()),
            storage: Storage::new(path),
        }))
    }

    fn create_thread(&self) -> Thread {
        loop {
            let id = Uuid::new_v4();
            let state = ThreadState::new();

            if self.0.threads.lock().unwrap().insert(id, state.clone()).is_none() {
                return Thread::new(id, self.clone(), state);
            }
        }
    }

    fn thread(&self, id: Uuid) -> Option<Thread> {
        self.state(id).map(|state| Thread::new(id, self.clone(), state))
    }

    fn commit(&self, id: Uuid, state: &ThreadState, message: impl Into<String>) -> io::Result<()> {
        let mut state = state.write().unwrap();
        let mut next = state.messages.clone();
        next.push(message.into());
        self.0.storage.commit(id, &next)?;
        state.messages = next;
        Ok(())
    }

    fn state(&self, id: Uuid) -> Option<ThreadState> {
        self.0.threads.lock().unwrap().get(&id).cloned()
    }
}

impl Storage {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self(Arc::new(StorageInner {
            path: path.into(),
            data: Mutex::new(HashMap::new()),
        }))
    }

    fn commit(&self, id: Uuid, messages: &[String]) -> io::Result<()> {
        let key = id.to_string();
        let mut data = self.0.data.lock().unwrap();
        let prev = data.insert(key.clone(), messages.to_vec());

        match fs::write(&self.0.path, serde_json::to_vec_pretty(&*data)?) {
            Ok(()) => Ok(()),
            Err(err) => {
                if let Some(prev) = prev {
                    data.insert(key, prev);
                } else {
                    data.remove(&key);
                }
                Err(err)
            }
        }
    }
}

impl Thread {
    fn new(id: Uuid, session: Session, state: ThreadState) -> Self {
        Self { id, session, state }
    }

    fn id(&self) -> Uuid {
        self.id
    }

    fn push(&self, message: impl Into<String>) -> io::Result<()> {
        self.session.commit(self.id, &self.state, message)
    }

    fn state(&self) -> &ThreadState {
        &self.state
    }
}

impl ThreadState {
    fn new() -> Self {
        Self(Arc::new(RwLock::new(ThreadStateInner::new())))
    }

    fn read(&self) -> LockResult<RwLockReadGuard<'_, ThreadStateInner>> {
        self.0.read()
    }

    fn write(&self) -> LockResult<RwLockWriteGuard<'_, ThreadStateInner>> {
        self.0.write()
    }
}

impl ThreadStateInner {
    fn new() -> Self {
        Self { messages: Vec::new() }
    }
}

fn main() {
    let session = Session::new("session.json");
    let thread = session.create_thread();

    thread.push("hello").unwrap();
    thread.push("world").unwrap();
    let same_thread = session.thread(thread.id()).unwrap();
    let state = thread.state().read().unwrap();
    println!("{:?}", state.messages);
    println!("{}", same_thread.id());
}

```

下面是这个架构的核心思想：
# Architecture

## Overview

Current roles:

- `Session`: owns thread registration and thread creation.
- `Thread`: lightweight handle for one thread.
- `ThreadState`: shared state object for one thread, protected as a whole by a lock.
- `Storage`: persists session data to local JSON.

Current persistence format:

```json
{
  "<thread-uuid>": ["message1", "message2"]
}
```

## Object Relationships

```text
Session
  -> Arc<SessionInner>
       -> Mutex<HashMap<Uuid, ThreadState>>
       -> Storage

Thread
  -> Uuid
  -> Session
  -> ThreadState

ThreadState
  -> Arc<RwLock<ThreadStateInner>>

Storage
  -> Arc<StorageInner>
       -> PathBuf
       -> Mutex<HashMap<String, Vec<String>>>
```

Important points:

- `Session` is the owner of thread identity and thread registration.
- `Session::create_thread()` allocates the `Uuid`.
- `Thread` does not own the global registry. It is only a handle.
- `ThreadState` is shared by cloning its internal `Arc`, so multiple handles to the same thread see the same state.
- `Storage` does not manage thread lifecycle. It only manages persisted data.

## Responsibilities

### Session

- Create new threads with `Session::create_thread()`.
- Keep the `Uuid -> ThreadState` mapping.
- Return an existing thread with `Session::thread(id)`.
- Coordinate the `push -> commit -> in-memory update` flow.

### Thread

- Represent one thread handle.
- Expose `id()`.
- Expose `push(message)`.
- Expose `state() -> &ThreadState` for read or write access to thread state through guards.

`Thread` is intentionally cheap. It carries enough information to operate on one thread without owning global storage.

### ThreadState

- Hold the mutable per-thread state.
- Protect the whole state with one `RwLock`.
- Expose `read()` and `write()` guards.

This is important: the lock is on `ThreadState` as a whole, not on `messages` alone. If more per-thread fields are added later, they should go into `ThreadStateInner` and stay under the same lock.

### Storage

- Maintain the persistence cache used for writing JSON.
- Serialize and write the full JSON file.
- Roll back the cache entry if file writing fails.

`Storage` is infrastructure, not the owner of thread identity.

## Push Semantics

Current `push` semantics are:

1. `thread.push(msg)` calls `Session::commit(thread_id, state, msg)`.
2. `Session::commit` takes the thread state's write lock.
3. It clones the current message list, appends the new message, and builds the next snapshot.
4. It asks `Storage::commit(thread_id, &next)` to persist that snapshot.
5. If persistence succeeds, the in-memory `ThreadState` is updated.
6. If persistence fails, the in-memory `ThreadState` is not updated.

So the invariant is:

- `push` returns successfully only after the new state has been written to disk.
- A failed `push` does not partially update the thread state in memory.

## Locking Model

Current locks:

- `SessionInner.threads: Mutex<HashMap<Uuid, ThreadState>>`
- `ThreadState: RwLock<ThreadStateInner>`
- `StorageInner.data: Mutex<HashMap<String, Vec<String>>>`

What each lock protects:

- `SessionInner.threads` protects the thread registry.
- `ThreadState` protects all fields of one thread's state.
- `StorageInner.data` protects the persisted in-memory snapshot before JSON is written.

### Lock Order

The important lock order is:

```text
ThreadState write lock -> Storage data lock
```

Lookup of a thread in `SessionInner.threads` happens separately and briefly.

Avoid introducing the reverse order:

```text
Storage data lock -> ThreadState lock
```

That reverse order would create deadlock risk once the code grows.

## Read Access

`Thread::state()` returns `&ThreadState`, not a cloned `Vec<String>`.

Reading is done by holding a read guard:

```rust
let state = thread.state().read().unwrap();
println!("{:?}", state.messages);
```

This means:

- no message clone is forced by the API,
- the caller decides how long the read lock is held,
- if the caller wants an owned copy, the caller can clone explicitly.

## Why `ThreadState` Uses `RwLock`

`RwLock` is used because:

- reads can use a shared lock,
- writes such as `push` use an exclusive lock,
- the API can safely expose a read view without returning invalid references.

Returning a bare `&Vec<String>` from inside locked state would be unsound because the lock would be released when the function returns.

## Current Tradeoffs

- `Storage::commit` rewrites the full JSON file each time.
- `Session::create_thread()` is responsible for generating UUIDs.
- `Thread` stores both `Session` and `ThreadState` so it can operate cheaply and still expose state directly.
- The design favors simple semantics and clear ownership over maximum write throughput.

## Practical Summary

- `Session` manages thread existence.
- `Thread` is a handle.
- `ThreadState` is the locked resource object.
- `Storage` writes snapshots to disk.
- `push` is atomic from the caller's perspective: success means disk and memory are aligned.


当然这只是一个demo 具体怎么结合到当前的项目进行重构还需要你来考虑

另外当前openjarvis项目Session本身的状态应该不需要持久化 每次从thread中加载就行了