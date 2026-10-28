# lab4 实验报告（第六章：文件系统与 I/O 重定向）

- 分支：ch6
- 实验目录：`os/`、`easy-fs/`
- 本地评测结果：`ci-user/check/ch6.py` → **Test passed: 31/31**，`RESULT: PASS (chapter 6)`（两轮独立运行均通过）

## 一、编程作业实现的功能（200 字以内）

在 easy-fs 的 `DiskInode` 中增加 `nlink` 字段（初始化为 1），为 `Inode` 记录自身 inode 号并提供 `stat/link/unlink`：`link` 在目录中追加指向同一 inode 的目录项并递增 `nlink`；`unlink` 用最后一个目录项填补被删项、缩小目录 size，递减 `nlink`，减到 0 时回收数据块与 inode。内核侧实现 `sys_fstat`（`File` trait 新增 `stat` 方法，OSInode 由 easy-fs 元数据填 `Stat`）、`sys_linkat`、`sys_unlinkat`，并补上 `sys_get_time`、`sys_mmap/sys_munmap`（按 port 位设 R/W/X、校验对齐与区间冲突）、`sys_spawn`（直接以 ELF 建新 TCB，不复制父地址空间）。

## 二、实现要点与验证

### 1. easy-fs 层

- `DiskInode` 新增 `nlink: u32`，`initialize()` 置 1；`DiskInode` 尺寸变化后 inode 区布局由 `size_of::<DiskInode>()` 统一算出，打包工具与内核共用同一份代码，磁盘镜像格式保持一致。
- `Inode::link(name, inode_id)`：先查重（同名返回失败），目标 inode 的 `nlink += 1`，再在目录尾部追加一个 32 字节目录项。
- `Inode::unlink(name)`：定位目录项下标与 inode 号 → `nlink -= 1` → 若为 0 则 `clear_size` 回收全部数据块并 `dealloc_inode` → 用最后一个目录项填洞并把目录 `size` 缩小一个 `DIRENT_SZ`（不重新分配块，逻辑简单且不会碎片化）。
- `Inode::stat()` 返回 `{ inode_id, nlink, is_dir }`；`EasyFileSystem::dealloc_inode()` 归还 inode 位图的位。

### 2. 内核层

- `sys_fstat`：从 `fd_table` 取 `Arc<dyn File>`，通过 `File::stat()`（默认实现返回 `None`，只由 `OSInode` 覆盖）拿到元数据，构造 `Stat { dev = 0, ino = inode_id, mode = FILE/DIR, nlink }`，用页表翻译后的用户缓冲区写回。
- `sys_linkat / sys_unlinkat`：`translated_str` 取路径后在 `ROOT_INODE` 上做 link/unlink；同名链接、文件不存在分别返回 -1。
- `sys_get_time`：读 `time` CSR 换算成 usec，按**逐页拷贝**写入用户 `TimeVal`，因此结构体被两页切开也不会写坏相邻物理页。
- `sys_mmap`：要求 start 页对齐、len 非 0、port ∈ {1,2,4} 的组合（bit0=R、bit1=W、bit2=X），并检查 `[start, ceil(start+len))` 与已有 `MapArea` 无重叠，否则 -1；`sys_munmap` 要求整段页对齐且**恰好**等于某个已映射区间，才摘除该 `MapArea`（否则 -1）。这样 `ch4_mmap3` 的「与已映射区重叠返回 -1」和 `ch4_unmap` 的「先 unmap 再 mmap 成功」都能满足。
- `sys_spawn`：读入 ELF 后由 `TaskControlBlock::spawn` 直接构造子进程（新 `MemorySet`、新 pid/kstack、独立 fd 表、父进程记录在 children 中），避免 `fork + exec` 复制整个地址空间的开销。

### 3. 验证

`ci-user/local_check.ps1 -Worktree .\.worktrees\ch6 -Chapter 6` 完整复刻官方流程：覆盖 `os/build.rs`（ch6 用分支自带版本）→ 编译 ch2..ch6 用户程序 → 把 `ch6_usertest.elf` 复制成内核要加载的 `ch6b_initproc.elf` → `easy-fs-fuse` 打包 `fs.img` → 编译内核 → QEMU 挂 `virtio-blk` 盘启动 → `check/ch6.py` 校验。

- 期望的 29 条 pattern 全部命中，2 条 not-expected 均未出现，合计 **31/31**。
- 串口输出末行为 `ch6 Usertests passed!`，随后 `[kernel] Idle process exit with exit_code 0`（正常收尾）。
- 前向兼容：ch2~ch5 的测例（`ch2b_*`、`ch3b_yield*`、`ch3_sleep*`、`ch4b_sbrk`、`ch4_mmap*`、`ch4_unmap*`、`ch5b_exit`、`ch5b_forktest*`、`ch5_spawn*`）在同一轮 `ch6_usertest` 中一并通过。

## 三、问答题

### ch6-1. easy-fs 中 root inode 起什么作用？如果它损坏了会发生什么？

easy-fs 没有多级目录，所有文件都平铺在根目录下，root inode（inode 号 0）就是这棵「一层目录树」的入口。它本身是一个 `DiskInodeType::Directory` 的磁盘 inode，它的数据块里存的是定长目录项数组：每项 32 字节 = 28 字节名字（27 字节名 + `\0`）+ `u32` inode 号。`Inode::find / create / link / unlink / ls` 都从 root inode 出发，按名字线性扫描目录项，拿到 inode 号后再用 `EasyFileSystem::get_disk_inode_pos` 算出该 inode 落在哪个块、块内偏移多少。

内核启动时 `EasyFileSystem::root_inode()` 固定按 inode 0 取根，`task::INITPROC` 通过 `open_file("ch6b_initproc")` 找到初始进程，其它程序也全靠 `open_file(name)` 从根目录拿。所以 root inode 一旦损坏，文件系统就失去唯一入口：

- 若 `size` 被写小/目录项名字被覆盖 → `find` 找不到任何程序，`INITPROC` 的 `unwrap()` 直接 panic，内核连第一个进程都起不来；
- 若 `size` 被写大而对应数据块没分配 → `read_at` 会读到未分配的垃圾块（或断言失败），目录项里的 inode 号变成随机值，`get_disk_inode_pos` 会定位到 inode 区之外的块，轻则读出错乱文件，重则写坏别的 inode / 位图；
- 若它的 `direct[]` 块号丢失 → 所有目录项不可见，整个磁盘上的文件全都"消失"（数据块其实还在，但没有任何索引能找到它们）。

因此 easy-fs 的可靠性完全依赖根 inode 与位图的一致性，这也是选做题里「日志式 easy-fs」要解决的问题。

### ch7-1. 举出使用 pipe 的一个实际应用的例子

最常用的是 shell 里的管道：`cat access.log | wc -l` 统计文件行数。shell 先建一个匿名管道，`fork` 出两个子进程：`cat` 把 stdout 重定向到管道写端，`wc` 把 stdin 重定向到读端，然后两个进程并发执行——`cat` 一边读文件一边往管道里写，`wc` 一边从管道读一边累计行数。内核的管道是一个环形缓冲区，写满时写进程阻塞、读空时读进程阻塞，`cat` 退出后写端引用计数归零，`wc` 读到 EOF（read 返回 0）就结束。整个过程不需要临时的中间文件，数据也不落盘。同样形式还有 `ps aux | grep rust`、`ls | sort | uniq`（多级管道）等。

### ch7-2. 多进程互相通信的机制设计

管道是「点对点 + 一次性」的，N 个进程两两通信要建 O(N²) 条管道，且没有名字、只能靠 fork 继承。更易用的做法是引入**有名字的内核通信对象**，我倾向于设计成「信箱 + 全局命名空间」：

1. **命名空间**：内核维护一张全局表，键为字符串名字（或 `(uid, name)`），值为信箱 id；进程用 `mailbox_open(name, create)` 取得 id，与文件描述符一样由内核分配句柄，权限用创建者 uid 校验。
2. **消息队列**：每个信箱是一条带边界的消息队列（不是字节流），发送 `mailbox_send(id, buf, len)` 一次投递一条消息，接收 `mailbox_recv(id, buf)` 取最早一条，保留消息边界，避免管道那种"要自己切分协议"的麻烦。队列满时按 flag 选择阻塞或返回 `EAGAIN`。
3. **多对多**：任意进程只要拿到同一个名字就能收发，天然支持 1:1、1:N（多个读者/写者共享一个信箱）、N:1；配合 `mailbox_close(id)` 引用计数回收，最后一个句柄关闭时释放队列。
4. **同步原语**：在信箱之上再提供 `select`/`epoll` 式的多路等待，或者把信箱和信号量/条件变量组合（ch8 的邮箱正是这个思路），这样进程不必忙等。

更进一步的方案是共享内存 + 信号量：由内核按名字创建一块共享页，双方 `shm_map` 到各自地址空间，用信号量保护读写指针——吞吐最高，但需要自己处理结构与同步，易用性不如消息队列。实际系统里通常两者并存：小控制消息走消息队列，大数据走共享内存。

## 四、荣誉准则

> 1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 **以下各位** 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：
>
>    **_《你交流的对象说明》：本次实验借助 AI 编程助手（DeepSeek Harness 中的 agent）完成代码实现与调试，本人审阅。_**
>
> 2. 此外，我也参考了 **以下资料** ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：
>
>    **_《你参考的资料说明》：rCore-Tutorial-Book-v3（https://rcore-os.cn/rCore-Tutorial-Book-v3/）、rCore-Tutorial-Guide（https://learningos.cn/rCore-Tutorial-Guide/）。_**
>
> 3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。
>
> 4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按"-100"分计。

## 五、（可选）对本次实验设计与难度的看法

- 本章的工作量主要不在系统调用本身，而在 easy-fs 的元数据设计：`nlink` 放在 `DiskInode` 里、`dealloc_inode` 归还位图、目录项删除后要不要搬移，都得先想清楚磁盘布局，否则测例能过但文件系统会慢慢烂掉。做完这些再看 `linkat` 的三个 syscall，其实只是薄薄一层封装。
- `fstat` 的实现方式受限于 `fd_table` 里存的是 `Arc<dyn File>`：直接向下转型（`Any`）很别扭，给 `File` trait 加一个带默认实现的 `stat()` 更干净。建议指导书提一句这种设计取舍。
- `mmap` 的测例（`ch4_mmap3` 与 `ch4_unmap`）实际隐含了「区间冲突检查」和「先 unmap 才能再 mmap 同一页」的语义，如果只按「插入 `MapArea`」实现，会在 `mmap3` 的 `mmap(start-len, len+1) == -1` 上翻车；这一点指导书说得比较含糊，容易踩坑。
- 建议仓库里自带的 Windows 本地评测脚本把 `easy-fs-fuse` 打包和 `ch<N>_usertest -> ch<N>b_initproc` 的重命名也写进去（本次已补齐），否则 ch6 起不来会让人误以为是内核 bug。
