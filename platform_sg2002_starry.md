# LicheeRV Nano（SG2002）上的 StarryOS 记录

这篇说明记录了在 LicheeRV Nano 上编译并启动 StarryOS `sg2002` 分支时，实际遇到的步骤和坑。

## 1. 选对仓库和分支

上游 `Starry-OS/StarryOS` 的 `main` 分支不够用，必须用 `sg2002` 分支：

```bash
git clone --recursive -b sg2002 https://github.com/pengzechen/StarryOS.git
```

如果你只克隆默认分支，可能拿不到 `rootfs/` 里的 patch 脚本，也没有 SG2002 专用的启动流程。

## 2. 本地路径依赖缺失

`sg2002` 分支依赖几个本地路径仓库，普通 clone 之后不会自动齐全。

### 2.1 缺少 `sg200x-bsp`

`cargo fetch` 会失败，因为 `api/Cargo.toml` 里引用了：

```toml
sg200x-bsp = { path = "../sg200x-bsp" }
```

处理办法：

```bash
cd /home/kingdom/StarryOS
git clone https://github.com/yfblock/sg200x-bsp.git
```

### 2.2 `dw_apb_uart` 路径不对

仓库里原本写的是：

```toml
dw_apb_uart = { path = "../yfblock/dw_apb_uart" }
```

这个路径在本机不存在，改成本地已经克隆的仓库即可：

```toml
dw_apb_uart = { path = "../dw_apb_uart" }
```

然后把驱动仓库克隆到本机：

```bash
cd /home/kingdom
git clone https://github.com/arceos-org/dw_apb_uart.git
```

## 3. 根文件系统镜像不在仓库里

`rootfs/ext4_100m.img` 没有提交到仓库里，必须先在本地创建。

做法如下：

```bash
cd /home/kingdom/StarryOS/rootfs
dd if=/dev/zero of=ext4_100m.img bs=1M count=100
mkfs.ext4 ext4_100m.img
```

然后修改 `rootfs.sh`，把 `BUSY_BOX_DIR` 指向你本机 BusyBox 的 `_install` 目录。

## 4. BusyBox 准备

先把 BusyBox 按 riscv64 编译好，再安装到 `_install`。
文档里那条短命令只是最后的安装步骤，默认你前面的配置和编译已经完成。

然后执行：

```bash
make ARCH=riscv CROSS_COMPILE=riscv64-linux-musl- CONFIG_PREFIX=$(pwd)/_install install
```

`rootfs.sh` 的作用是挂载 `ext4_100m.img`，把 BusyBox 文件树拷进去，再加入 `init.sh`。

## 5. `sg2002` 分支需要打 patch

`sg2002` 分支在 `rootfs/` 目录下带了三个本地 patch 脚本：

```bash
./rootfs/patch_sstatus.sh
./rootfs/patch_table_entry.sh
./rootfs/patch_uspace.sh
```

它们会修改 `~/.cargo` 下缓存的 crate 源码。

### 常见失败

`patch_sstatus.sh` 可能报：

```text
❌ sstatus.rs not found
```

这通常表示 Cargo 源码还没下载下来。
先把本地路径依赖补齐，再运行 `cargo fetch`。

## 6. 编译问题和修法

### 6.1 `DW8250` 没有 `init_with_baud`

StarryOS 代码里会调用：

```rust
uart.init_with_baud(baud);
```

但本地 `dw_apb_uart` 只有 `init()`。

解决方法：给 `dw_apb_uart` 补 `init_with_baud()`，并让 `init()` 直接调用 `115200` 版本。

### 6.2 `Pinmux` 没有 `set_uart2`

SG2002 代码里会用到：

```rust
pinmux.set_uart2();
```

但本地 `sg200x-bsp` 只提供了 `set_uart1()`。

解决方法：给 `sg200x-bsp` 补 `set_uart2()`，映射关系如下：

- `PWR_GPIO0 -> UART2_TX`
- `PWR_GPIO1 -> UART2_RX`

## 7. Nightly / `axio` 兼容问题

`make sg2002` 一开始在 `axio` 里报了：

```text
use of unstable library feature `unsigned_signed_diff`
```

解决方法：在缓存的 crate 里补 feature gate：

```rust
#![feature(unsigned_signed_diff)]
```

位置是：

```text
~/.cargo/registry/src/.../axio-0.3.0-pre.1/src/lib.rs
```

## 8. `make sg2002` 最后一步的 objdump 失败不影响产物

构建已经生成了：

- `StarryOS_sg2002.bin`
- `StarryOS_sg2002.elf`
- `rootfs/ext4_100m.img`

最后失败的只有：

```text
riscv64-linux-musl-objdump: not found
```

这只会影响 `asm.txt` 的反汇编输出，不影响内核镜像本身。

## 9. 启动分区大小

板子原始的 boot FAT 分区太小。
这条流程里建议至少扩到 `256MB`。

`512MB` 也没问题。

## 10. 运行时 panic：`CVSD init failed`

第一次启动时 panic 是：

```text
CVSD init failed: Io
```

原因是 `Cargo.toml` 里启用了 `driver-cvsd`：

```toml
sg2002 = ["dep:axplat-riscv64-sg2002", "axfeat/driver-cvsd"]
```

对于这里这条“SD 卡里放镜像文件”的流程，应该换成 ramdisk 路线：

```toml
sg2002 = ["dep:axplat-riscv64-sg2002", "axfeat/driver-ramdisk"]
```

这比去探测真实的 CVSD block 设备更符合当前流程。

## 11. 最终启动命令

把下面两个文件复制到 FAT 启动分区后：

- `StarryOS_sg2002.bin`
- `ext4_100m.img`

在 U-Boot 里执行：

```bash
setenv starry 'fatload mmc 0:1 0x89000000 ext4_100m.img; fatload mmc 0:1 0x80200000 StarryOS_sg2002.bin; go 0x80200000'
run starry
```
