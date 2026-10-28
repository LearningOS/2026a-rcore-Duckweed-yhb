# rCore-Tutorial 实验报告 · lab1（第三章 多道程序与分时多任务）

本章在 ch3 框架上补全了系统调用 `sys_trace`，其余功能沿用框架既有的分时多任务实现，测例判定结果为 Test passed: 7/7。

## 一、实现的功能

在第三章框架上实现了 `sys_trace`（调用号 410）：`trace_request` 为 0 时把 `id` 当作用户指针读出一字节，为 1 时向该地址写入 `data` 的最低字节，为 2 时返回当前任务对指定系统调用的累计次数，其他取值返回 -1。计数器是 `TaskControlBlock` 里新增的 `[usize; 500]` 数组，在 `trap_handler` 的 UserEnvCall 分支、`syscall()` 分派之前用 a7 自增，使任务查询 TRACE 自身时也把本次调用算进去。第三章还没有地址空间，trace 的读写直接解引用物理地址，没有做安全检查。CI 判定 Test passed: 7/7。

## 二、编程作业：sys_trace

### 2.1 三种 request 的语义

实现落在 `os/src/syscall/process.rs`：

```rust
0 => unsafe { (id as *const u8).read_volatile() as isize },
1 => { unsafe { (id as *mut u8).write_volatile(data as u8) }; 0 }
2 => get_syscall_times(id) as isize,
```

- `trace_request = 0`：`id` 按 `*const u8` 解释，`read_volatile` 读一个字节并零扩展成 `isize` 返回，`data` 被忽略。用 volatile 是必要的，否则编译器可以把这次读当成死代码消掉。
- `trace_request = 1`：`id` 按 `*mut u8` 解释，`data as u8` 只保留最低 8 位写入，返回 0。
- `trace_request = 2`：返回当前任务的计数，`id` 是系统调用号。这一次 TRACE 调用本身也要算进去。
- 其他取值返回 -1。

第三章没有页表，用户态的 VA 就是 PA，所以 `id as *const u8` 直接解引用是合法的。串口日志里 `bad instruction = 0x804003a4` 这种打印形式也印证了这一点：内核拿到的 sepc 是应用被装载的物理地址（app base 0x80400000 加 0x3a4），不是虚拟地址。

### 2.2 计数器记在哪

`os/src/task/task.rs` 给 `TaskControlBlock` 加了一个按调用号索引的数组：

```rust
pub const MAX_SYSCALL_NUM: usize = 500;
pub syscall_times: [usize; MAX_SYSCALL_NUM],
```

放 TCB 里是因为统计的粒度是"当前任务"，任务退出时整块 TCB 一起消失，不用额外清理，也不会把上一个任务的计数带给下一个。`os/src/task/mod.rs` 里给 `TaskManager` 加了 `inc_syscall_times` / `get_syscall_times`，都通过 `inner.exclusive_access()` 拿到 `current_task` 再读写对应的那一格；`syscall_id >= MAX_SYSCALL_NUM` 时直接忽略（读返回 0）。

500 这个上界不是随手取的：本实验里 `SYSCALL_TRACE = 410`，数组必须覆盖到 410，否则 request=2 查 TRACE 会被判成越界而恒返回 0。代价是每个 TCB 多 500 × 8 = 4000 字节，第三章的 TCB 是 `[TaskControlBlock; MAX_APP_NUM]`（`MAX_APP_NUM = 16`）静态数组，合计 64 KB 静态内存。

### 2.3 为什么在 trap_handler 里统计

自增位置在 `os/src/trap/mod.rs` 的 UserEnvCall 分支：

```rust
cx.sepc += 4;
inc_syscall_times(cx.x[17]);          // x[17] 就是 a7，系统调用号
cx.x[10] = syscall(cx.x[17], [cx.x[10], cx.x[11], cx.x[12]]) as usize;
```

放在 `syscall()` 之前是语义要求，不是风格偏好。`ch3_trace.rs` 第 24 行断言 `assert_eq!(2, count_syscall(SYSCALL_TRACE))`：程序里第一次 `count_syscall(GETTIMEOFDAY)` 是第 1 次 TRACE，第二次 `count_syscall(TRACE)` 自身是第 2 次，内核返回给它的必须已经是 2。若在 `syscall()` 返回之后再统计，或等到 `sys_trace` 内部读完计数再自增，这里都会得到 1，断言失败。

另外两点：在 `trap_handler` 里 `cx.x[17]`（a7）还没被任何东西改写，这是能同时拿到"调用号"和"当前任务"的位置；`syscall()` 对未知调用号直接 `panic!`，把统计放在它的分派之前，未实现的调用号也会被计数，不会因为 panic 路径漏记。

## 三、简答作业

### 3.1 三个 bad 测例的行为

使用的 SBI 从启动日志可见：`RustSBI version 0.4.1`，平台 `riscv-virtio,qemu`，1 个 hart，内存范围 `0x80000000 - 0x88000000`。

三个测例依次运行，串口输出为：

```
[kernel] PageFault in application, bad addr = 0x0, bad instruction = 0x804003a4, kernel killed it.
[kernel] IllegalInstruction in application, kernel killed it.
[kernel] IllegalInstruction in application, kernel killed it.
```

**ch2b_bad_address**：用户程序执行 `(0x0 as *mut u8).write_volatile(0)`，在 U 态向地址 0 写。0x0 没有映射，硬件报 StorePageFault（scause = 15），`stval = 0x0`、`sepc = 0x804003a4`。内核的 `StoreFault | StorePageFault` 分支打印上面第一行后调用 `exit_current_and_run_next()` 杀掉该任务，程序里紧跟的 `panic!("FAIL")` 没有机会执行。

**ch2b_bad_instructions**：程序执行 `core::arch::asm!("sret")`。sret 是 S 态返回指令，U 态执行属于特权指令违规，硬件报 IllegalInstruction（scause = 2）。内核打印 `IllegalInstruction in application, kernel killed it.`。

**ch2b_bad_register**：程序执行 `csrr {}, sstatus` 读 S 态 CSR，同样是非法指令，输出与上一个测例完全一样的一行。

两个 IllegalInstruction 行字面相同，是因为内核的 IllegalInstruction 分支只打印固定字符串，既没有打印 `stval`，也没有打印 `sepc`。要区分"执行了 sret"和"读了 sstatus"，需要把这两项也打出来：`stval` 一般是非法指令的编码（也可能为 0），`sepc` 是出错指令的地址。这恰好说明：U 态下 S 态特权指令与 S 态 CSR 访问都被硬件拦在同一类异常里，用户程序无法借此读到 `sstatus` 的内容。

三个程序都被杀掉之后，下一个应用照常输出 `Hello, world from user mode program!`，后续 power/sleep/trace 测例继续跑完，说明 U 态的越权操作只终止当前任务，不影响内核和其他任务。

### 3.2 trap.S 中 __alltraps / __restore 的七个问题

以下行号对应本分支的 `os/src/trap/trap.S`（共 61 行）。

**（1）L40 刚进入 `__restore` 时 `sp` 的值，以及两种使用情景**

`sp` 指向内核栈上刚由 `__alltraps` 压好的那份 `TrapContext` 的基址，即 `addi sp, sp, -34*8` 之后的那个地址；此时 `sscratch` 里存的是用户栈顶。它有两种进入方式：

- 任务第一次被调度运行时：`TaskContext::goto_restore(init_app_cx(i))` 把 `ra` 设成 `__restore`、`sp` 设成预置了 `TrapContext` 的内核栈，`__switch` 切过去后直接从 `__restore` 开始执行；
- 陷处理返回时：`__alltraps` 末尾 `call trap_handler` 返回后顺序落到 L40（`__restore` 标签紧跟在 `call` 之后），`sp` 仍然指着同一个 `TrapContext`。

**（2）L43-L48 特殊处理的寄存器及其对进入用户态的意义**

`t0` 取 `32*8(sp)`、`t1` 取 `33*8(sp)`、`t2` 取 `2*8(sp)`，随后分别写回 `sstatus`、`sepc`、`sscratch`。

- `sstatus`：`sret` 依据 `SPP` 决定返回后的特权级（本章为 0，回到 U 态），依据 `SPIE` 决定返回后 `SIE` 是否置 1（用户程序要能被下一次时钟中断打断），此外还带着 `SUM` 等位。不恢复它，sret 会按内核的 sstatus 返回，可能回到 S 态。
- `sepc`：`sret` 后 PC 的取值。不恢复就会从内核里的地址继续取指。
- `sscratch`：保存用户栈顶，供下一次从 U 态陷入时 L13 的 `csrrw` 换回内核栈。

**（3）L50-L56 为何跳过 x2 和 x4**

x2 是 `sp`。这十几行正在用 `sp` 当基址遍历 `TrapContext`，此时不能把用户栈顶装进 `sp`，否则后续的 `ld xN, N*8(sp)` 全部错位。用户 sp 的恢复走了另一条路：L45 已经把它读进 `t2` 并写进 `sscratch`，最后由 L60 的 `csrrw` 换到 `sp`。

x4 是 `tp`。本实验的用户程序（`no_std` + `user_lib`）不使用线程指针，`__alltraps` 也没有保存 tp（L21 的注释写明 "skip tp(x4), application does not use it"），恢复它既没有数据来源也没有意义；进入时不存、返回时不取，各省一条访存。

**（4）L60 之后 `sp` 和 `sscratch` 的值**

`csrrw sp, sscratch, sp` 执行前：`sp` = 内核栈顶（`TrapContext` 之后），`sscratch` = 用户栈顶。执行后两者互换：`sp` = 用户栈顶，`sscratch` = 内核栈顶。`sret` 之后用户程序就在自己的栈上继续跑；`sscratch` 里留着内核栈顶，等下一次陷入时由 L13 再换回来。

**（5）状态切换发生在哪条指令**

在 L61 `sret`。`sret` 是 S 态的返回指令，硬件执行时做四件事：PC ← `sepc`，特权级 ← `sstatus.SPP`，`SIE` ← `SPIE`，`SPP` ← 0，同时清 `SPIE`。此时 `sstatus` 和 `sepc` 已在 L43-L48 恢复成用户程序的值，`SPP = 0`，所以这条指令一执行，CPU 就在 U 态从用户的 sepc 处取指。

**（6）L13 之后 `sp` 和 `sscratch` 的值**

`__alltraps` 入口的 `csrrw sp, sscratch, sp` 执行前：`sp` = 用户栈顶，`sscratch` = 内核栈顶。执行后：`sp` = 内核栈顶（紧接着 `addi sp, sp, -34*8` 就在这块内核栈上分配 `TrapContext`），`sscratch` = 用户栈顶（L34-L35 再从 `sscratch` 把它读出并存进 `2*8(sp)`）。

**（7）从 U 态进入 S 态是哪条指令**

是用户程序执行的 `ecall`（`user/src/syscall.rs` 里 `asm!("ecall", ...)`，a7 传调用号、a0-a2 传参数）。U 态执行 `ecall` 时硬件把 `scause` 置为 8（Environment call from U-mode）、`sepc` 置为这条 `ecall` 自身的地址（内核对 `cx.sepc += 4` 跳过它）、PC 置为 `stvec` 指向的 `__alltraps`，特权级升到 S。时钟中断从 U 态打断也会发生同样的 U→S 切换，但本实验里被测例驱动的是 `ecall` 这条路径。

## 四、荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 **以下各位** 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

   > 本次实验借助 AI 编程助手（DeepSeek Harness 中的 agent）完成代码实现与调试，本人审阅代码与结论。

2. 此外，我也参考了 **以下资料**，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

   > rCore-Tutorial-Book-v3、rCore-Tutorial-Guide 以及 Rust/riscv crate 文档。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按"-100"分计。

## 五、难度与工作量

`sys_trace` 本身工作量不大：三种 request 加起来不到 20 行，主要时间花在搞清楚"本次调用也计入统计"这件事上——先把计数放到 `sys_trace` 内部，`assert_eq!(2, count_syscall(TRACE))` 会稳定地返回 1，改成在 `trap_handler` 分派前自增才对上。计数数组的上界也容易被忽略，取小了 TRACE（410）查不到自己的计数。

简答题第 2 题的价值在于逼着把 `__alltraps` / `__restore` 和 sscratch 的双重身份真正对上：两条 `csrrw sp, sscratch, sp` 方向相反、作用不同，只有把"用户栈顶"和"内核栈顶"在两个时刻各在哪一边写清楚，才算读懂这段汇编。

对实验设计的一点建议：第三章没有地址空间，`sys_trace` 的读写是裸解引用，稍不注意就会把内核写坏；官方说明里也承认这一点。如果能在测例里加一个"越界 id 应返回 -1 而不是崩溃"的用例，边界条件会更明确。
