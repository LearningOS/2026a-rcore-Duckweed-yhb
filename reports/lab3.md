# rCore-Tutorial 实验报告 · lab3（第五章 进程及进程管理）

## 一、实现的功能（200 字以内）

在 ch4 基础上补齐系统调用并新增进程管理：`sys_get_time` 经页表逐字段写入、支持跨页；`mmap`/`munmap` 校验对齐、prot 与区间重叠后建/拆映射；`spawn` 用 ELF 直接建新进程，不复制父进程空间；实现 stride 调度与 `set_priority`（优先级 ≥2，初始 16，pass = BIG_STRIDE/prio）。

## 二、测试结果（本地实测）

### 2.1 学生评测脚本（check/ch5.py）

```
& .\ci-user\local_check.ps1 -Worktree .\.worktrees\ch5 -Chapter 5 -TimeoutSec 180
```

结果：`Test passed: 15/15`，`RESULT: PASS (chapter 5)`。

15 条中 13 条是期望出现（`get_time OK!`、`Test sleep OK!`、`Test sleep1 passed!`、`Test 04_1 OK!`、`Test 04_4 test OK!`、`Test 04_5 ummap OK!`、`Test 04_6 ummap2 OK!`、`Test spawn0 OK!`、`Test wait OK!`、`Test waitpid OK!`、`Test set_priority OK!` 等），另 2 条是期望不出现（`FAIL: T.T`、`Test sbrk failed!`）。本轮串口日志里可见 `ch5_usertest` 打印 `ch5 Usertests passed!`，随后 initproc 正常退出。

### 2.2 官方 CI 的等价路径（check/ch5b.py）

官方 `ci-user/Makefile` 在 ch5 会把 `ch5b_usertest.elf` 复制为 `ch5b_initproc.elf` 作为初始进程。按同样的方式在本机复现后，用 `check/ch5b.py` 校验：`Test passed: 17/17`，检查器退出码 0。

### 2.3 stride 公平性

`ch5_stride` 一次完整运行的输出（`ch5_stride0`–`ch5_stride5`，每个跑满约 4000ms）：

| priority | 运行计数 | 计数/优先级 |
| --- | --- | --- |
| 5 | 6,519,600 | 1,303,920 |
| 6 | 7,952,800 | 1,325,466 |
| 7 | 9,456,000 | 1,350,857 |
| 8 | 10,493,600 | 1,311,700 |
| 9 | 11,924,400 | 1,324,933 |
| 10 | 13,094,400 | 1,309,440 |

计数与优先级基本成正比；`max(计数/prio) / min(计数/prio) = 1,350,857 / 1,303,920 ≈ 1.036 < 1.5`，满足实验要求。

## 三、问答题：stride 算法深入

### 3.1 8 bit 无符号 stride 的溢出例子

p1.stride = 255，p2.stride = 250，pass 均为 10。按算法应先调度 p2：p2.stride 变为 250 + 10 = 260，用 8 bit 无符号存储溢出为 260 − 256 = 4。此时若直接用无符号整数比较，p2(4) < p1(255)，调度器会再次选中 p2，而按真实值 255 < 260 本应轮到 p1。所以**实际情况不是轮到 p1**：p2 会连续多跑若干个时间片，直到它的 stride 再次绕到 255 之后，公平性被破坏。

原因是溢出把环上的大小关系翻转了：无符号比较只在“真实差值小于半个量程”时才与真实先后一致，而 255 与 260 的差虽然很小，存储值 255 与 4 的无符号比较却给出了相反结论。

### 3.2 为什么 STRIDE_MAX − STRIDE_MIN ≤ BigStride / 2

记同一时刻所有可运行进程 stride 的最大、最小值为 M、m。初始全为 0，差为 0。每次调度选中最小者，把它的 stride 加上自己的 pass；因为优先级 priority ≥ 2，所以 pass = BigStride / priority ≤ BigStride / 2。

归纳：设调度前 M − m ≤ BigStride / 2。被选中的进程原值为 m，调度后新值 ≤ m + BigStride / 2；其余进程不变，仍落在 [m, M]。stride 只增不减，故新的最小值 m′ ≥ m；新的最大值 M′ ≤ max(M, m + BigStride / 2) ≤ m + BigStride / 2（因为 M ≤ m + BigStride / 2）。于是

M′ − m′ ≤ (m + BigStride / 2) − m = BigStride / 2。

归纳成立。直观地说：优先级 ≥ 2 限制了单次步长不超过半个 BigStride，任何进程都不可能比当前最小者领先超过半个 BigStride。

### 3.3 支持溢出比较的 `partial_cmp`

```rust
use core::cmp::Ordering;

struct Stride(u64);

impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // 模 2^64 意义下，other 在 self 前方不超过半个环，就说明 self < other。
        // 8 bit 存储时把半区间换成 128（BigStride = 255）。
        if other.0.wrapping_sub(self.0) < (1u64 << 63) {
            Some(Ordering::Less)
        } else {
            Some(Ordering::Greater)
        }
    }
}

impl PartialEq for Stride {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}
```

判定依据：在模 2^n 的环上，若两点真实距离小于半量程，则 `other − self` 的环绕差值就反映了先后。题中 8 bit、BigStride = 255 时半区间为 128：

- 125 与 255：255 − 125 = 130 ≥ 128，判为 125 > 255（对应提示中的 `(125 < 255) == false`）；
- 129 与 255：255 − 129 = 126 < 128，判为 129 < 255（对应 `(129 < 255) == true`）。

又由 3.2 已知任意两个 stride 的真实极差不超过 BigStride / 2（远小于半量程），因此真实差值一定小于半个环，这个比较器不会产生歧义。题目已约定两个 Stride 永不相等，`PartialEq` 恒为 false，故 `partial_cmp` 只返回 `Less` / `Greater`，不会返回 `Equal`。

## 四、荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 **以下各位** 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

   > 本次实验借助 AI 编程助手（DeepSeek Harness 中的 agent）完成代码实现与调试，本人审阅。

2. 此外，我也参考了 **以下资料** ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

   > rCore-Tutorial-Book-v3（https://rcore-os.github.io/rCore-Tutorial-Book-v3/）；rCore-Tutorial-Guide（https://learningos.github.io/rCore-Tutorial-Guide/）。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。

## 五、可选：对本次实验的看法

本章工作量适中，但有两个地方容易踩坑：一是 `sys_get_time` 必须经页表写、且要按字段分别写，直接解引用用户虚拟地址会写坏内核或触发页错误；二是 `mmap` 的合法性判断（页对齐、prot 位、与已有 area 重叠）要和 `mmap1`–`mmap3`、`unmap2` 的期望逐条对上，重叠检查要用页号区间而不是字节区间。stride 调度本身逻辑不复杂，难在“优先级高 → pass 小 → 更常被选中”这一反直觉的点，以及 `BIG_STRIDE` 与整数除法误差的取舍。相比 FIFO，stride 让不同优先级的进程运行时间有了可测量的差异，实验设计很直观。
