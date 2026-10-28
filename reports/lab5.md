# 实验五：并发（chapter8）实验报告

## 一、实验概述

本实验在 rCore-Tutorial ch8 框架上完成。框架已给出内核态线程管理（`TaskControlBlock` /
`ProcessControlBlock` 的多线程扩展）、`thread_create` / `waittid`、`Mutex`（自旋锁与阻塞锁）、
`Semaphore`、`Condvar` 以及管道等机制，本次工作集中在：

1. **死锁检测**（本次实验的编程作业）：实现 `sys_enable_deadlock_detect`，并让 `mutex_lock` /
   `semaphore_down` 在检测到死锁时拒绝请求、返回 `-0xDEAD`。
2. **补齐前几章的遗留系统调用**（本章不参与评分，但要使分支完整）：`sys_get_time`、`sys_mmap`、
   `sys_munmap`、`sys_spawn`。

最终官方评测脚本 `ci-user/check/ch8.py` 结果：**Test passed: 25/25，RESULT: PASS (chapter 8)**。

完成本次实验所用时间：（待本人按实际投入填写）约 \_\_ 小时，其中阅读框架与调试占大部分，
真正的编码量约 150 行。

## 二、功能实现简述（200 字内）

为每个进程在 PCB 中维护一张资源分配图：记录每把锁/每个信号量的持有线程集合，以及每个线程当前
阻塞在哪个资源上。在 `mutex_lock` / `semaphore_down` 决定阻塞之前，从申请者出发，
「线程 → 资源 → 持有者线程」递归回溯：只要某个持有者仍可运行（它没有等待边），申请就迟早能
满足；若所有持有者都陷入等待，并且回溯回到了申请者自身，就判定死锁，返回 `-0xDEAD` 而不阻塞。
锁的 `unlock` / 信号量的 `up` 在唤醒等待者时把所有权直接转交给被唤醒线程，并清除其等待边；
线程退出时清掉它的全部图节点，避免线程号复用时残留旧边。检测开关是进程私有的，只有
`enable_deadlock_detect(1)` 之后才做判断，因此未开启检测的进程（如 `mpsc_sem` 等）完全不受影响。
此外补齐了 `get_time`（跨页拷贝）、`mmap` / `munmap` 与 `spawn`。

## 三、死锁检测的设计（重点）

### 3.1 为什么不用课本上的银行家算法

练习文档给出的示例算法是银行家算法（Available / Allocation / Need 三个矩阵 + 安全性检查）。
它需要知道**每个线程未来还会申请多少资源**（Need 矩阵），而内核在 `mutex_lock` / `semaphore_down`
的那一刻根本无法预知用户程序后续要申请什么；银行家算法本质上是**预防**（在分配前保证系统始终
处于安全状态，会保守地拒绝很多本来能成功的请求），而不是**发现**。
因此我采用的是**资源分配图（等待图）上的环路检测**：只有在「此刻这条请求一定会永远等下去」时
才拒绝，既满足测例对 `sem2`（无环）不得误报的要求，也符合练习里「检测到死锁就拒绝该次请求」
的语义。

### 3.2 数据结构（每个进程一份，放在 PCB 里）

```rust
pub struct DeadlockGraph {
    enabled: bool,                              // 是否开启检测
    mutex_holders: Vec<Vec<usize>>,             // 互斥锁 id -> 持有它的线程号集合
    sem_holders:   Vec<Vec<usize>>,             // 信号量 id -> 持有它的线程号集合
    waiting:       Vec<Option<(ResKind, usize)>>, // 线程号 -> 它阻塞在哪个资源上
}
```

- 资源用「种类 + 该进程资源表里的 id」标识（`ResKind::Mutex` / `ResKind::Sem`）。锁和信号量
  在 PCB 里是两张不同的表，id 会重复，必须带上种类，否则会互相混淆。
- 持有者用线程号（tid）而不是 TCB 指针：tid 在进程内唯一，且不依赖对象地址是否被复用。
- 用 `Vec` 而不是矩阵：资源数量是动态的，而且一个线程可能持有同一信号量的多个单位
  （计数信号量），所以持有者列表允许重复，释放时只删掉一个。

### 3.3 何时判环

只在「这次请求确实会阻塞」时才判：`MutexBlocking::lock` 先读一次 `locked`，
`Semaphore::down` 先看 `count <= 0`；若不会阻塞就正常获取，不做任何图上的判断。
这样既省时间，也避免把「能立刻拿到资源」误判成死锁。

### 3.4 判环算法

申请者 `tid` 申请资源 `R`（已经确定会阻塞）时，判断「这次申请是否可能被满足」：

```text
can_be_satisfied(R):
    holders = R 的持有者集合
    if holders 为空: return true            # 没有记录到持有者，不武断判死锁
    for h in holders:
        if h == tid: continue               # 它（间接）在等我，不可能释放给我
        if h 没有等待边: return true        # h 还能运行，迟早会释放，我一定能拿到
        # h 自己被阻塞在资源 R2 上，只有当 R2 也永远无法满足时，h 才永远不会释放
        if R2 已在当前路径上: continue      # 与申请者无关的环，保守地当作不可解
        把 R2 压入路径
        if can_be_satisfied(R2): return true
        弹出 R2
    return false                             # 所有持有者都不可能释放

would_deadlock(R) = !can_be_satisfied(R)
```

关键点在于「**只要有一个持有者还能运行，申请就能满足**」。这正是区分
`ch8_deadlock_sem1`（成环，且环上每个持有者都在等待 → 判死锁）与
`ch8_deadlock_sem2`（申请者要的资源虽然被两个线程持有，但其中一个持有者还能运行、
之后会释放 → 不判死锁）的地方。若简单地「沿持有者一直走，走回自己就报死锁」，
`sem2` 会误报——因为它是多实例资源，申请者只需要其中一个单位。

`ch8_deadlock_mutex1`（同一线程重复加同一把 blocking 锁）就是 `holders == {tid}` 的退化情形，
第一步就命中 `h == tid`，返回 `-0xDEAD`。

### 3.5 图的维护（所有权转移与清理）

- **获得**：`lock` 一次成功、`down` 一次成功（`count >= 0` 那条路径）都把当前线程加进持有者列表，
  同时清掉它自己的等待边。
- **释放 / 唤醒**：`unlock` 若队列非空，锁的所有权**直接转交**给被唤醒线程（`locked` 保持为
  `true`），图上做 `release(我) + acquire(被唤醒者)`；队列为空才真正把 `locked` 置 `false` 并
  `release(我)`。信号量的 `up` 同理：`count <= 0` 时唤醒一个等待者，把这一单位的所有权转交给它，
  并从我的持有记录里删掉一个单位。**等待边在被唤醒的瞬间清除**，而不是等它被调度运行之后，
  否则「已经拿到资源但还没运行」的线程会被误认为仍在等待，从而产生假环。
- **线程退出**：`exit_current_and_run_next` 里调用 `forget_thread(tid)`，清掉等待边并把该 tid
  从所有持有者列表里删除。因为退出时 `res` 会被丢弃、tid 会被回收，如果不清，复用同一个 tid 的
  新线程就会继承到旧的图边，产生假环。

### 3.6 为什么检测开关和整张图都是「每个进程」的

内核里 `mutex` / `semaphore` 都保存在 PCB 的 `mutex_list` / `semaphore_list` 里，本来就是
进程私有的；如果图是全局的，不同进程的线程会出现在同一张图里，完全没有意义。
更实际的原因是**误报**：`ch8b_mpsc_sem` 的消费者线程会持有 `SEM_EXISTED` 的全部单位，
然后自己阻塞在 `SEM_EXISTED` 上（它消费完才释放，但释放的是 `SEM_EMPTY`），
这在图上就是「自己等自己持有」的自环。如果开关是全局的、并且这些测例与 3 个 deadlock 测例
被 `ch8_usertest` 并发 fork 出来同时运行，`mpsc_sem` 就会被误杀。
把 `enabled` 放进 PCB，并在所有图上操作前检查它（未开启时连图都不维护），
就保证了「没开启检测的进程零影响」。这也是练习文档说的「允许进程按需开启或关闭」。

### 3.7 已知的局限（不掩盖）

- 按练习要求，只检测 mutex 之间、信号量之间的死锁，不考虑两者混用以及和 `waittid` 混用
  造成的死锁；`waittid` 里等待子线程退出没有纳入这张图。
- `MutexSpin` 是循环重试的自旋/让出式实现，不会进入阻塞状态，因此不纳入检测。若对自旋锁
  自我重复加锁仍会死循环。
- 计数信号量的「持有者」是记录哪些线程各拿走过一个单位，如果用户把 `up` 用在
  和自己 `down` 不配对的对象上（例如生产者/消费者互相 `up`），持有记录会退回近似集合语义；
  这不影响正确性方向的判断（最坏情况是保守地不报死锁），但不如银行家算法严格。
- 判定是「安全」的偏保守实现：遇到与申请者无关的环时选择放弃继续深入（返回「可满足」），
  即宁可漏报也不误报。

## 四、本次补齐的其他系统调用

| 系统调用 | 实现要点 |
| --- | --- |
| `sys_get_time` | 用 `get_time_us()` 取出开机以来的微秒数，拆成 `sec` / `usec`；写入用户地址时用 `translated_byte_buffer` 逐页拷贝，因此 `TimeVal` 跨页也不会出错 |
| `sys_mmap` | `MemorySet::mmap`：要求按页对齐、`len` 非空、`port` 只含 R/W/X 三位、且不与该进程已有 `MapArea` 重叠，否则返回 -1；通过 `insert_framed_area` 建立带 U 权限的按需映射 |
| `sys_munmap` | `MemorySet::munmap`：只接受与该进程某个 `MapArea` 完全一致的区间（部分匹配拒绝），`unmap` 后执行 `sfence.vma` 刷新 TLB |
| `sys_spawn` | 从文件系统读出 ELF，`ProcessControlBlock::spawn` 复用 `ProcessControlBlock::new` 构造新地址空间与主线程，再补上父子链接；与 `fork+exec` 的区别是父进程不会在自己身上执行子程序、子进程不复制父进程地址空间 |

说明：`sys_set_priority` 仍然返回 -1。本章的调度器是普通 FIFO 就绪队列，`ProcessControlBlockInner`
里没有任何优先级字段，它属于第 5 章 stride 调度实验的内容；ch8 的模板保留了这个系统调用号却
没有相应数据结构，强行实现需要把 stride 调度一并移植，超出本章范围，故保留为不支持并在注释中说明。
`sys_fstat` / `sys_linkat` / `sys_unlinkat` 也仍返回 -1：它们需要 easy-fs 增加 inode 的硬链接
计数（`nlink`）、目录项删除与 inode 回收，属于第 6 章实验对 easy-fs 的改造。练习文档明确说明
「本次实验不要求合并之前的实验内容（除 `sys_get_time` 以外）」，本章评测
（`check/ch8.py`；其中 `ch6b_filetest_simple` 只用到 `open/write/read/close`）也不涉及这三个调用，
因此没有移植；`sys_get_time` 是明确要求且被 `sleep()` 隐式依赖的，所以补齐了。

## 五、测试与验证

评测方式（Windows 本地复现官方 CI）：

```powershell
& .\ci-user\local_check.ps1 -Worktree .\.worktrees\ch8 -Chapter 8 -TimeoutSec 300
```

脚本等价于 `ci-user/Makefile`：编 ch2..ch8 的用户程序 → 用 easy-fs-fuse 打包 `fs.img`
→ `cargo build` 内核 → 带 virtio-blk 挂盘跑 QEMU → 把串口输出喂给官方 `check/ch8.py`。
补测过程中还发现并修复了本地脚本的两处问题：ch8 不应覆盖 `os/build.rs`（ch6 之后的程序来自
`fs.img`，不是编进内核的 `link_app.S`），以及 QEMU 需要追加 `-drive/-device virtio-blk-device`
参数。

串口末尾可以看到每个测例的退出码：

```
Usertests: Test ch8_deadlock_mutex1 in Process 48 exited with code 0
Usertests: Test ch8_deadlock_sem1 in Process 49 exited with code 0
Usertests: Test ch8_deadlock_sem2 in Process 53 exited with code 0
ch8 Usertests passed!
[kernel] Idle process exit with exit_code 0 ...
```

三个死锁测例的行为符合预期：
- `mutex1`：第二次 `mutex_lock` 得到 `-0xDEAD`，`assert_eq!` 通过；
- `sem1`：成环被检出，至少一个线程 `semaphore_down` 返回 `-0xDEAD` 并 `exit(-1)`，
  主线程统计到 `failed > 0`；被拒线程退出时释放资源，级联唤醒其余线程，测试不会卡住；
- `sem2`：无环、无任何拒绝，4 个子线程全部 exit 0，主线程 `assert_eq!(failed, 0)` 通过。

调试过程中的一个真实坑：`sem1` / `sem2` 一开始永远跑不完。原因是用户库的 `sleep()` 用
`get_time()` 轮询，而 `sys_get_time` 此时返回 -1，导致 `while get_time() < start + period`
永远成立、主线程卡在自旋里，永远不去释放 barrier 信号量。把 `sys_get_time` 补上之后，
两个测例立刻正常——这也说明「死锁检测没生效」和「测试根本没走到检测那一步」要分开排查。

## 六、问答作业

### 问题 1：主线程（0 号线程）退出时，需要回收哪些资源？其他线程的 TCB 可能在哪些位置被引用，分别是否需要回收，为什么？

**需要回收的资源**（与 `exit_current_and_run_next` 里 tid == 0 的分支对应）：

1. 所有线程的用户资源：tid（`RecycleAllocator` 里的 `dealloc_tid`）、每个线程的用户栈
   `MapArea`（连同其物理页）与 trap context 所在的那一页。
2. 每个线程的内核栈：`KernelStack` 的 Drop 会从 `KERNEL_SPACE` 中 `remove_area_with_start_vpn`
   撤销内核栈映射，并把 kstack id 还给分配器。
3. 整个地址空间：`memory_set.recycle_data_pages()` 清空所有 `MapArea`，页表帧随之释放。
4. 文件描述符表 `fd_table`（关闭所有打开的文件/管道）、信号标志 `signals`。
5. 进程自己持有的互斥锁、信号量、条件变量（`mutex_list` / `semaphore_list` / `condvar_list`），
   它们随 PCB 一起析构，其中的等待队列（`Arc<TaskControlBlock>`）也随之释放。
6. 进程控制的子进程链表 `children`、以及 pid（`PidHandle` 的 Drop 把 pid 还给分配器，
   并从 `PID2PCB` 中摘除）。

**其他线程 TCB 可能被引用的位置，以及是否/为何要回收**：

1. **PCB 的 `tasks` 数组**：属于本进程，进程销毁时 `tasks.clear()` 一起释放，不需要单独处理。
2. **调度器的就绪队列 `TASK_MANAGER`**：Ready 状态的线程在这里。**必须显式移除**（`remove_task`），
   否则它会在进程销毁后被调度，访问已释放的地址空间。
3. **定时器堆 `TIMERS`**：正在 `sys_sleep` 的线程在这里。**必须显式移除**（`remove_timer`），
   否则到期时会被 `wakeup_task` 唤醒一个已死的任务。
4. **各同步原语的等待队列**（`MutexBlockingInner::wait_queue`、`SemaphoreInner::wait_queue`、
   `CondvarInner::wait_queue`）：这些对象都属于该进程，PCB 释放时一起销毁，里面的
   `Arc<TaskControlBlock>` 也随之丢弃，因此**不需要**单独回收。
5. **`TaskManager::stop_task`**：保存「刚刚退出、内核栈可能还在使用」的那个任务。**不能立即回收**，
   它的作用就是延长一个 Arc 的生命周期，避免在 `__switch` 还在用它的内核栈时释放。
   下一次有任务退出时覆盖即可（单核情况下旧的那个此时确实已经不再使用内核栈）。
6. **栈上的临时 `Arc`**（例如 `sys_waittid` 里克隆的、`current_task()` 返回的）：随函数返回释放，
   不需要特殊处理。

所以真正需要「主动」回收的只有第 2、3 项——即那些**会自己按时间或调度再次访问 TCB** 的全局结构。

### 问题 2：对比两种 `Mutex` 实现，二者有什么区别？这些区别可能会导致什么问题？

**区别一：`lock` 是否在循环里重新检查条件。**
`Mutex1` 用 `loop {}`，被唤醒后会重新检查 `locked`；`Mutex2` 只用一次 `if`，被唤醒后直接
认为已经拿到锁。

**区别二：`unlock` 是否把锁的所有权「转交」给被唤醒者。**
`Mutex1` 先把 `locked = false`，再 `add_task(唤醒者)`——锁在 `unlock` 返回时是**空闲**的，
被唤醒者只是被放进就绪队列，之后还要自己去抢。
`Mutex2` 在队列非空时**保持 `locked = true`**，直接把锁交给队首线程，只有队列为空时才置 false。

**可能的问题：**

- `Mutex1` 存在「被插队」（barging）问题：`unlock` 之后、被唤醒者真正运行之前，任何新来的
  线程都可以直接抢到锁；被唤醒者醒来发现锁又被人拿走了，只能再次入队并重新睡眠。
  由于它有 `loop` 重新检查，**不会破坏互斥性、也不会丢唤醒**，但会多出一次无谓的
  阻塞/唤醒，并且被插队的线程从队首挪到队尾，可能被长期饿死（公平性差）。
- `Mutex2` 靠「锁的所有权直接转交」避免了插队，FIFO 语义和公平性更好，也不会出现
  「醒来看见锁又被抢走」的浪费。但它成立的前提是**只有 `unlock` 会唤醒这个等待队列**：
  一旦发生虚假唤醒（spurious wakeup，或者该队列被别的路径 `wakeup_task` 唤醒），
  它没有 `loop` 重新检查，就会在 `locked` 仍然为 `true`（别人持有）的情况下进入临界区，
  **破坏互斥**，而且 `unlock` 里的 `assert!(locked)` 之后还可能把锁状态弄乱。
- 补充一点：`Mutex1` 的 `lock` 在「循环重试」时若把当前任务重新压入 `wait_queue`，
  队列里会出现同一任务的多个条目，`unlock` 可能连续唤醒同一个任务；由于有 `loop` 兜底，
  不会死锁，但队列会变长、调度开销变大。

本实验采用的实现属于 `Mutex2` 的形态（所有权转交 + 单次判断），因为它同时满足了条件变量
`Condvar::wait`（`unlock` 后阻塞、被 `signal` 唤醒后重新 `lock`）和死锁检测（等待边在唤醒瞬间
清除、持有者在同一时刻转移给被唤醒者）的需要。

## 七、荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 **以下各位** 就（与本次实验相关的）
   以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

   > 本次实验借助 AI 编程助手（DeepSeek Harness 中的 agent）完成代码实现与调试，本人审阅。

2. 此外，我也参考了 **以下资料**，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

   > rCore-Tutorial-Book-v3（<https://rcore-os.cn/rCore-Tutorial-Book-v3/>）、
   > rCore-Tutorial-Guide（<https://learningos.cn/rCore-Tutorial-Guide/>）。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。我清楚地知道，从以上方面
   获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。我未曾也不会向他人
   （含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。我提交至本实验的评测
   系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。我清楚地知道，以上情况均为本课程
   纪律所禁止，若违反，对应的实验成绩将按“-100”分计。

## 八、对实验设计与难度/工作量的看法（选填）

- 本章的框架改动比较大（线程、锁、信号量、条件变量、管道一起出现），但真正要写的代码不多，
  练习文档说「约 100 行」，实际统计（含注释）约 150 行，其中判环本身不到 60 行，
  难点不在代码量而在**把语义想清楚**：什么时候算「阻塞」、等待边什么时候建立/清除、
  多实例资源为什么不能简单地判环。
- `ch8_deadlock_sem2` 这个「不许误报」的测例设计得很好：它会淘汰「沿持有者一路走、回到自己
  就报死锁」的朴素实现，逼着实现者去区分「持有者还能不能运行」。
- 建议改进：练习文档给出的银行家算法与「在 `mutex_lock`/`semaphore_down` 时检测」这个要求
  其实并不完全匹配（Need 矩阵内核无法得知），如果能明确说明「也可以采用资源分配图判环」，
  会减少很多困惑。
- 另外，`sem1`/`sem2` 这两个测例依赖用户库的 `sleep()`，而 `sleep()` 又依赖 `sys_get_time`，
  但练习里只说「不要求合并之前实验内容（除 `sys_get_time` 以外）」——等于这个隐藏依赖是
  必须补的，这一点比较容易被忽略（本实验就是在这里卡了一轮）。
