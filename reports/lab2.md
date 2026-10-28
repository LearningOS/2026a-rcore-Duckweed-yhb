# rCore-Tutorial 实验报告 · lab2（第四章 地址空间）

本章在 ch4 框架上重写了 `sys_get_time` 与 `sys_trace` 让它们通过页表访问用户内存，并实现了 `mmap` / `munmap` 两个匿名映射系统调用，测例判定结果为 Test passed: 16/16。

## 一、实现的功能

在第四章框架上重写了两个系统调用：`sys_get_time` 先把 `TimeVal` 经当前任务页表逐页写出（能处理跨页），`sys_trace` 的读/写 request 通过页表手工翻译，地址未映射或权限不符时返回 -1。新增 `mmap`（222）与 `munmap`（215）：检查页对齐、prot 位掩码与区间重叠后按 `MapPermission` 建立映射；munmap 按页取消映射。`sys_trace` 的计数逻辑沿用第三章，仍在 `trap_handler` 分派前自增。测例判定 Test passed: 16/16。

## 二、编程作业

### 2.1 重写 sys_get_time

第四章的用户指针是虚拟地址，内核直接 `*ts = ...` 会把 `0x1000` 之类的用户 VA 当物理地址写，轻则写坏内核数据，重则触发内核页错误。`TimeVal` 是两个 `usize`（16 字节），可能横跨两个物理页，所以必须按页处理。

实现顺序是：先算时间，再 `check_user_range(token, ts as usize, 16, true)` 逐页确认"已映射且可写"，最后用 `translated_byte_buffer` 把目标区间切成若干物理页片段逐字节拷贝：

```rust
let token = current_user_token();
if !check_user_range(token, ts as usize, len, true) { return -1; }
for buf in translated_byte_buffer(token, ts as *const u8, len) { ... }
```

检查必须在前。`translated_byte_buffer` 内部对 `page_table.translate(vpn).unwrap()`，遇到未映射的页会直接把内核 panic 掉；`check_user_range` 先挡掉这种输入，`sys_get_time` 才可能按规范返回 -1。

### 2.2 重写 sys_trace

`trace_request = 0/1` 通过 `user_byte_ptr(token, id, write)` 完成一次手工翻译：取 `PageTable::from_token(token)`，对 `VirtAddr::from(addr).floor()` 求 PTE，确认 `is_valid()` 且权限满足（读查 R，写查 W），再用 `pte.ppn().get_bytes_array()` 加页内偏移 `addr % PAGE_SIZE` 得到内核可以直接解引用的物理指针。返回 -1 时用的是 `isize` 的 -1，不是 `u8` 的 0xFF。`trace_request = 2` 读计数器，不再经过页表。

两处与第三章不同：

- **权限检查**。第 3 章裸解引用，第 4 章必须按页表项判断。`ch4_trace1.rs` 先 `mmap(0x10000000, 4096, prot=1)`（只读），随后 `assert!(None != trace_read(start))`、`assert_eq!(-1, trace_write(start))`：读能过、写返回 -1，靠的就是这一步。
- **SV39 位宽检查**。`addr >= 1 << 39` 直接返回 -1（`SV39_LIMIT`）。这一条不是装饰：`VirtAddr::from(usize)` 内部是 `v & ((1 << 39) - 1)`，高位会被静默截断，`0x7FFF_FFFF_FFFF_FFFF` 会被折算成某个真实用户页。测例正是用 `isize::MAX` 验证这一点，期望读到 `None`、写返回 -1。

`0x80200000`（内核起始物理地址）也返回 -1，原因不是地址范围判断，而是**用户页表里根本没有内核映射**：`translate` 在用户页表里走三级找不到有效 PTE，返回 `None`。这从侧面说明双页表的隔离是生效的。

### 2.3 mmap / munmap

**合法性检查**（`os/src/mm/memory_set.rs` 的 `MemorySet::mmap`）按顺序做四件事：

```rust
if start_va % PAGE_SIZE != 0 || port == 0 || port & !0b111 != 0 { return false; }
let end_vpn: VirtPageNum = VirtAddr::from(start_va.wrapping_add(len)).ceil();
if end_vpn <= start_vpn { return false; }
```

- 起始地址必须按页对齐：`start % PAGE_SIZE != 0` 直接拒绝；
- prot 只允许低 3 位：`port & !0b111 != 0` 拒绝，`port == 0`（既不读也不写不执行）也拒绝；
- 长度为 0 或长度回绕（`start + len` 溢出成更小的地址）时 `end_vpn <= start_vpn`，拒绝；
- **重叠检测**：遍历 `self.areas`，用虚拟页区间判断 `area.start < end_vpn && start_vpn < area.end`，任意一个已有区域相交就返回 -1。`ch4_mmap3.rs` 的四条断言依次是：先映射 `[0x10000000, 0x10001000)`，再请求 `mmap(0x0FFFF000, 0x1001)`（尾页与已有区域相交，重叠）、`mmap(0x10001001, 0x1000)`（起始不对齐）、`mmap(0x10001000, 0x1000, 0)`（prot 为 0）、`mmap(0x10001000, 0x1000, prot | 8)`（prot 高位非零），四条都由上面几行覆盖。重叠检查不只是规范要求，`PageTable::map` 里有 `assert!(!pte.is_valid(), "vpn is mapped before mapping")`，漏判会直接把内核 panic 掉而不是返回 -1。

**权限位换算**。用户视角的 prot 是 bit0 = 可读、bit1 = 可写、bit2 = 可执行；而内核 `MapPermission` 的 R/W/X 在 bit1/bit2/bit3（`MapPermission { R = 1<<1, W = 1<<2, X = 1<<3, U = 1<<4 }`），正好整体高一位，所以：

```rust
let permission = MapPermission::from_bits_truncate((port << 1) as u8) | MapPermission::U;
```

`port << 1` 把三个权限位对齐到 PTE 的位置，`| MapPermission::U` 置 PTE_U —— 少了这一位，映射建好了但 U 态一访问就是 PageFault，测例会以 `bad addr = 0x10000000` 被杀掉。V 位不用手工加：`MemorySet::mmap` 调 `insert_framed_area` 时建一个 `MapArea` 并 `push`，`MapArea::map` 逐页 `map_one`，`MapType::Framed` 分支里 `frame_alloc` 取物理帧，最后由 `PageTable::map` 用 `flags | PTEFlags::V` 落位（`os/src/mm/page_table.rs:141`）。

**prot 的一个副作用值得记一笔**：`prot = 2`（只写）会得到 R = 0、W = 1 的页表项，RISC-V 规定这是保留编码，硬件在第一次读时就报异常。串口日志里 `Test 04_1 OK!` 之后紧跟的两行

```
[kernel] PageFault in application, bad addr = 0x10000000, bad instruction = 0x42e, kernel killed it.
[kernel] PageFault in application, bad addr = 0x10000000, bad instruction = 0x42c, kernel killed it.
```

对应 `ch4_mmap1`（向只读页写，StorePageFault）和 `ch4_mmap2`（读只写页，LoadPageFault）这两个故意触发缺页的测例，它们不打印 OK 才算通过。能让它们只杀掉自己而不带崩内核，是 `trap_handler` 同时处理了 `LoadFault | LoadPageFault` 的结果——第三章只处理 Store 类，读缺页会落到 `_ => panic!`。

**munmap**：`start` 必须页对齐，然后在 `areas` 里找 `vpn_range` 恰好等于 `[floor(start), ceil(start+len))` 的区域，找到就 `remove` 并 `area.unmap(&mut page_table)` 逐页撤销映射（`MapType::Framed` 的帧随 `data_frames` 一起释放），返回 0；找不到返回 -1。`ch4_unmap2.rs` 的两种错误请求——`munmap(start, len+1)`（多出 1 字节，尾部多一页未映射）和 `munmap(start+1, len-1)`（起始不对齐）——都落到"找不到"分支返回 -1。

**两处与规范不一致的地方**（测例没覆盖，记录在此）：

- 官方写 `len` 可以为 0，但本实现 `len == 0` 时 `end_vpn <= start_vpn` 成立，返回 -1；
- munmap 只接受整块区域。真正部分取消映射（例如先 mmap 3 页再 munmap 中间 1 页）虽然不含未映射页，仍会返回 -1，因为实现要求 `vpn_range` 完全相等，而没有做 MapArea 拆分。

另外 `mmap` 没有像 `sys_trace` 那样做 SV39 上界检查，`start` 超过 `1 << 39` 时 `VirtAddr::from` 会把高位截断，请求会落到别处而不是返回 -1；`frame_alloc().unwrap()` 在物理内存不足时是 panic 而不是返回 -1，尽管规范把"物理内存不足"列为错误情况。

### 2.4 计数器

与第三章同一套：`TaskControlBlock` 加 `syscall_times: [usize; MAX_SYSCALL_NUM]`（500），`trap_handler` 的 UserEnvCall 分支在 `syscall()` 分派前 `inc_syscall_times(cx.x[17])`。这样做仍然是"本次调用也计入统计"的要求：`ch3_trace.rs` 的 `assert_eq!(2, count_syscall(TRACE))` 只有在分派前自增时才成立。第四章新增的 `TaskControlBlock::new` 里也补了 `syscall_times: [0; MAX_SYSCALL_NUM]`，否则新任务带的是未初始化内存。

## 三、问答作业

### 3.1 SV39 页表项的组成与标志位

页表项是 64 位，按位从高到低：`[63:54]` 保留（其中 63 位被 Svnapot、62:61 被 Svpbmt 占用，本项目不用）、`[53:10]` 物理页号 PPN（44 位）、`[9:8]` RSW（留给操作系统）、`[7]` D 脏位、`[6]` A 访问位、`[5]` G 全局位、`[4]` U 用户位、`[3]` X 可执行、`[2]` W 可写、`[1]` R 可读、`[0]` V 有效。本项目的 `PageTableEntry::new` 写的是 `bits = ppn.0 << 10 | flags.bits as usize`，`PTEFlags` 覆盖 V/R/W/X/U/G/A/D 八位，与规范一致。

各标志位的作用：

- **V**：该项是否有效。V = 0 表示这一级没有映射，页表遍历到此失败，访问触发缺页。这是 Lazy 分配和 swap 都能利用的位。
- **R/W/X**：允许的访问类型。取指要求 X，取数要求 R，存数要求 W。R = 0 且 W = 1 是保留编码，硬件视为非法，`ch4_mmap2` 的 PageFault 就来自这里。R = 1、W = 0 的页对 S 态可写（S 态写只读页在未开 sstatus.SUM 等条件下允许），但对 U 态只读。
- **U**：为 1 时 U 态可访问；为 0 时只有 S 态能访问。这正是双页表里内核页对用户不可见的硬件依据，也是 `mmap` 必须 `| MapPermission::U` 的原因。
- **G**：全局映射，切换 satp 时不清除对应 TLB 项，适合内核公共映射（本项目没有使用，任务切换时靠 `sfence.vma` 全刷）。
- **A/D**：访问位与脏位，供页面置换算法判断冷热。本项目没有开启硬件置位，仅在建表时由软件给出。
- **RSW**：硬件不用，留给内核标记 swap 项、COW 等自定义状态。

SV39 的虚拟地址是 39 位：`[38:30]` VPN[2]、`[29:21]` VPN[1]、`[20:12]` VPN[0]、`[11:0]` 页内偏移，每级 512 项；`satp` 的 MODE 写 8、PPN 字段放根页表物理页号。

### 3.2 缺页

**哪些异常可能是缺页导致的**：InstructionPageFault（scause = 12，取指缺页）、LoadPageFault（13，取数缺页）、StorePageFault（15，存数缺页）。三个 access fault（1/5/7）通常是权限或物理访问错误，一般不按缺页处理。本内核的 `trap_handler` 处理 `StoreFault | StorePageFault | LoadFault | LoadPageFault`，`InstructionPageFault` 会掉进 `_ => panic!("Unsupported trap ...")`，当前实现里取指缺页会把内核 panic 掉，这是一个缺口。

**发生缺页时相关寄存器的值**：

- `scause` = 12/13/15，区分取指、读、写；
- `stval` = 触发访问的虚拟地址。串口日志里 `bad addr = 0x10000000`（ch4_mmap1/2 访问的映射起点）和 `bad addr = 0xc000`（sbrk 回收后再写）都是它；
- `sepc` = 出错指令的地址，日志里 `bad instruction = 0x42e`、`0x5c0` 是用户虚拟地址（第三章同一现象打印的是 `0x804003a4` 这样的物理地址，两章对照可以看出地址空间已经生效）；
- `sstatus`：`SPP = 0` 说明陷来自 U 态，`SUM` 决定内核能否访问用户页；
- `satp`：当前任务的页表根，缺页处理要在这张表上补映射；此外就是 PTE 本身的状态（V = 0，或 R/W/X 与访问类型不匹配）。

**Lazy 策略的好处**：进程启动时只登记"这段虚拟区间应该有映射"（MapArea），不分配物理帧、不写 PTE，启动阶段就没有页表操作；程序实际只用到一小部分时，没碰过的页一帧物理内存都不占（申请 10G 只用 1M 的情况，代价从"整段 10G 的页表和物理帧"降到只付实际用到的 1M）；fork 也能推迟到写时再复制。

**10G 连续内存的页表开销估算**：10 GiB / 4 KiB = 2.5M 页。每个叶子 PTE 8 字节，2.5M × 8 B = 20 MiB，刚好装满 2.5M / 512 = 5120 个二级页表页（也是 20 MiB）；一级页表 5120 / 512 = 10 个（40 KiB）；根页表 1 个（4 KiB）。合计约 40 MiB 量级，即"几十 MB"，真正的 10 GiB 物理帧才是无法承受的部分。

**如何实现 Lazy**：`mmap` 只把 MapArea 记进 `MemorySet.areas`（记住 prot 和"这段是 Framed 待分配"），不建 PTE。缺页时用 `stval` 定位所属 MapArea（找不到说明越界，直接杀进程），检查 prot 是否允许本次访问（写要求 W，取指要求 X，不符也杀），再分配物理帧、按 prot 建 PTE，返回后 `sret` 重试刚才那条指令（`sepc` 保持不变，不能像 syscall 那样 +4）。关键是区分"未分配"和"越界"：只有落在某个 area 内、且权限允许的缺页才补页。

**swap 时页面失效在 PTE 上如何表现**：PTE 的 V 位清 0，使该页在页表中"无效"，访问即缺页；同时把 PPN 字段改存 swap 槽号（或 swap 区偏移），并用 RSW 位标记"这是一条 swap 项"，以区别于从未映射过的全 0 项。缺页处理按槽号把页换入、重建 PTE、置 V。

### 3.3 双页表与单页表

**单页表情况下如何更换页表**：内核与用户共用一张页表，`satp` 只在进程切换时更换——不同进程有不同的页表根；trap 进入内核和返回用户态都不需要动 `satp`。

**单页表下如何控制用户态无法访问内核页面**：靠 PTE 的 U 位。内核页 U = 0，U 态访问就触发 PageFault；硬件不看"当前是哪个页表"，只看页表项的 U 位。另外 `sstatus.SUM` 控制 S 态能否访问 U = 1 的页，与这条机制配合使用。

**单页表的优势**：trap 进出不用写 `satp` + `sfence.vma`，省掉每次系统调用的页表切换和 TLB 失效开销；内核可以直接解引用用户指针（不必像本实验那样用 `translated_byte_buffer` 按页拷贝、也不必先 `check_user_range` 手工查表），`sys_get_time` / `sys_trace` 会退回第三章那种直白写法；不需要 trampoline 映射，`TRAMPOLINE` / `TRAP_CONTEXT` 这套地址技巧可以省掉。

**双页表实现下何时更换页表**：每次 trap 进出各换一次。进入内核时 `trap.S` 的 `__alltraps` 从 `TrapContext` 的 `34*8(sp)` 取出内核 satp，执行 `csrw satp, t0; sfence.vma`（trap.S:41-42）；返回用户态时 `__restore` 用 a1 传入的用户 token 执行 `csrw satp, a1; sfence.vma`（trap.S:49-50）。任务切换本身（`__switch`）不换页表，因为所有任务共用同一张内核页表。

**如果写单页表内核，我会选择何时更换**：只在进程切换之后换一次，`satp` 写成新进程的页表根；trap 的进出都不换，内核页在同一张表里以 U = 0 的形式存在。代价是要接受 KPTI 所防的那种侧信道风险——双页表的根本理由就是让用户态页表里根本不含内核映射。

## 四、荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 **以下各位** 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

   > 本次实验借助 AI 编程助手（DeepSeek Harness 中的 agent）完成代码实现与调试，本人审阅代码与结论。

2. 此外，我也参考了 **以下资料**，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

   > rCore-Tutorial-Book-v3、rCore-Tutorial-Guide 以及 Rust/riscv crate 文档。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按"-100"分计。

## 五、难度与工作量

工作量主要在 `mmap` / `munmap` 的正确性判断上，而不在代码量（两个函数加上 `sys_` 包装不到 100 行）。容易踩坑的点有四个：prot 与 `MapPermission` 差一位，忘了 `| MapPermission::U` 会让映射"建好了但用不了"；munmap 的区间要和 MapArea 的 `vpn_range` 对齐比较，而不是拿字节区间算；`translated_byte_buffer` 里的 `unwrap` 要求先做范围检查；`VirtAddr::from` 对高位静默截断，所以 SV39 上界必须自己判。

问答部分的第二、三题需要把页表项、`satp`、`sstatus` 和 KPTI 的动机串起来想，特别是"单页表靠 U 位而不是靠换页表"这一点，和本项目 `mmap` 里必须置 U 位的实现是同一件事的两面。

对实验设计的建议：缺页处理目前只覆盖 Store/Load 两类，`InstructionPageFault` 会 panic 内核，测例里也没有取指缺页的用例；另外 `mmap` 的"物理内存不足返回 -1"在框架的 `frame_alloc().unwrap()` 下无法实现，规范与框架在这一处有冲突，建议在题目里说明或改成可失败的分配接口。
