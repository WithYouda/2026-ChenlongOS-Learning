//! Serial TTY device (`/dev/ttySx`)
//!
//! Provides raw UART byte-pipe devices that behave like Linux serial ports.
//! All protocol logic should live in userspace; the kernel only handles
//! hardware I/O, interrupt-driven RX buffering, and termios configuration.

use alloc::collections::vec_deque::VecDeque;
use core::{any::Any, task::Context};

use axerrno::{AxError, LinuxError};
use axfs_ng_vfs::{NodeFlags, VfsResult};
use axhal::mem::phys_to_virt;
use axpoll::{IoEvents, PollSet, Pollable};
use axsync::Mutex;
use axtask::future::{block_on, poll_io};
use bytemuck::AnyBitPattern;
use dw_apb_uart::DW8250;
use kspin::SpinNoIrq;
use memory_addr::{PhysAddr, pa};
use sg200x_bsp::pinmux::Pinmux;
use starry_core::vfs::DeviceOps;
use starry_vm::{VmMutPtr, VmPtr};

// ─── UART physical addresses (SG2002) ────────────────────────────────────────
const UART1_PADDR: PhysAddr = pa!(0x04150000);
const UART2_PADDR: PhysAddr = pa!(0x04160000);

// ─── UART IRQ numbers (SG2002 PLIC: UART0=44 .. UART4=48) ──────────────────
const UART1_IRQ: usize = 45;
const UART2_IRQ: usize = 46;

// ─── Ring-buffer capacity ────────────────────────────────────────────────────
const RX_BUF_CAP: usize = 4096;

// ─── Static per-port buffers and poll sets (required for fn() IRQ handlers) ──

static UART1_RX_BUF: SpinNoIrq<VecDeque<u8>> = SpinNoIrq::new(VecDeque::new());
static UART2_RX_BUF: SpinNoIrq<VecDeque<u8>> = SpinNoIrq::new(VecDeque::new());
static UART1_POLL: PollSet = PollSet::new();
static UART2_POLL: PollSet = PollSet::new();

/// Generic IRQ handler that drains a UART FIFO into a static buffer.
fn uart_irq_handler(paddr: PhysAddr, buf: &SpinNoIrq<VecDeque<u8>>, poll: &PollSet) {
    let mut uart = DW8250::new(phys_to_virt(paddr).as_usize());
    let mut rx = buf.lock();
    let mut got_data = false;
    loop {
        if let Some(c) = uart.getchar() {
            if rx.len() < RX_BUF_CAP {
                rx.push_back(c);
            }
            got_data = true;
        } else {
            break;
        }
    }
    uart.set_ier(true);
    drop(rx);
    if got_data {
        poll.wake();
    }
}

fn uart1_irq_handler() {
    uart_irq_handler(UART1_PADDR, &UART1_RX_BUF, &UART1_POLL);
}

fn uart2_irq_handler() {
    uart_irq_handler(UART2_PADDR, &UART2_RX_BUF, &UART2_POLL);
}

// ─── Termios (raw-mode only, mirrors kernel_termios) ─────────────────────────

/// Minimal termios matching `struct termios` layout (riscv64 linux).
#[repr(C)]
#[derive(Clone, Copy, AnyBitPattern)]
struct RawTermios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 19],
}

/// Minimal termios2 matching `struct termios2` layout.
#[repr(C)]
#[derive(Clone, Copy, AnyBitPattern)]
struct RawTermios2 {
    base: RawTermios,
    c_ispeed: u32,
    c_ospeed: u32,
}

impl RawTermios {
    /// Return a raw-mode termios (all processing disabled).
    fn raw(baud_cflag: u32) -> Self {
        // cflag: CS8 | CREAD | baud bits
        Self {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0o000060 /* CS8 */ | 0o000200 /* CREAD */ | baud_cflag,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0; 19],
        }
    }
}

impl RawTermios2 {
    fn new(base: RawTermios, speed: u32) -> Self {
        Self {
            base,
            c_ispeed: speed,
            c_ospeed: speed,
        }
    }

    fn speed(&self) -> u32 {
        self.c_ospeed
    }
}

// ─── WindowSize ──────────────────────────────────────────────────────────────
#[repr(C)]
#[derive(Clone, Copy, AnyBitPattern)]
struct WinSize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

impl Default for WinSize {
    fn default() -> Self {
        Self {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

// ─── Per-port mutable state ──────────────────────────────────────────────────

struct SerialConfig {
    termios2: RawTermios2,
    winsize: WinSize,
}

// ─── TtySerial device ────────────────────────────────────────────────────────

pub struct TtySerial {
    paddr: PhysAddr,
    irq: usize,
    rx_buf: &'static SpinNoIrq<VecDeque<u8>>,
    poll_set: &'static PollSet,
    config: Mutex<SerialConfig>,
}

impl TtySerial {
    /// Create a new TtySerial for the given UART.
    /// `baud` is the initial baud rate (e.g. 115200 or 1500000).
    /// The caller is responsible for pin-mux configuration.
    fn new(
        paddr: PhysAddr,
        irq: usize,
        baud: u32,
        rx_buf: &'static SpinNoIrq<VecDeque<u8>>,
        poll_set: &'static PollSet,
        irq_handler: fn(),
    ) -> Self {
        // Initialise hardware
        let vaddr = phys_to_virt(paddr).as_usize();
        let mut uart = DW8250::new(vaddr);
        uart.init_with_baud(baud);
        uart.set_ier(true);

        // Register IRQ
        axhal::irq::register(irq, irq_handler);
        axhal::irq::set_enable(irq, true);

        let termios2 = RawTermios2::new(RawTermios::raw(0), baud);

        Self {
            paddr,
            irq,
            rx_buf,
            poll_set,
            config: Mutex::new(SerialConfig {
                termios2,
                winsize: WinSize::default(),
            }),
        }
    }

    /// Re-configure the hardware baud rate.
    fn set_baud(&self, baud: u32) {
        let vaddr = phys_to_virt(self.paddr).as_usize();
        let mut uart = DW8250::new(vaddr);
        uart.init_with_baud(baud);
        uart.set_ier(true);
        axhal::irq::set_enable(self.irq, true);
    }
}

// ─── DeviceOps ───────────────────────────────────────────────────────────────

impl DeviceOps for TtySerial {
    fn read_at(&self, buf: &mut [u8], _offset: u64) -> VfsResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        block_on(poll_io(self, IoEvents::IN, false, || {
            let mut rx = self.rx_buf.lock();
            if rx.is_empty() {
                return Err(AxError::WouldBlock);
            }
            let n = buf.len().min(rx.len());
            for i in 0..n {
                buf[i] = rx.pop_front().unwrap();
            }
            Ok(n)
        }))
    }

    fn write_at(&self, buf: &[u8], _offset: u64) -> VfsResult<usize> {
        let vaddr = phys_to_virt(self.paddr).as_usize();
        let mut uart = DW8250::new(vaddr);
        for &b in buf {
            uart.putchar(b);
        }
        Ok(buf.len())
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> VfsResult<usize> {
        use linux_raw_sys::ioctl::*;
        match cmd {
            TCGETS => {
                let cfg = self.config.lock();
                (arg as *mut RawTermios).vm_write(cfg.termios2.base)?;
            }
            TCGETS2 => {
                let cfg = self.config.lock();
                (arg as *mut RawTermios2).vm_write(cfg.termios2)?;
            }
            TCSETS | TCSETSF | TCSETSW => {
                let new_termios: RawTermios = (arg as *const RawTermios).vm_read()?;
                let mut cfg = self.config.lock();
                let speed = cfg.termios2.speed();
                cfg.termios2 = RawTermios2::new(new_termios, speed);
                if cmd == TCSETSF {
                    self.rx_buf.lock().clear();
                }
            }
            TCSETS2 | TCSETSF2 | TCSETSW2 => {
                let new_termios2: RawTermios2 = (arg as *const RawTermios2).vm_read()?;
                let old_speed = self.config.lock().termios2.speed();
                let new_speed = new_termios2.speed();
                {
                    let mut cfg = self.config.lock();
                    cfg.termios2 = new_termios2;
                    if cmd == TCSETSF2 {
                        self.rx_buf.lock().clear();
                    }
                }
                if new_speed != 0 && new_speed != old_speed {
                    self.set_baud(new_speed);
                }
            }
            TIOCGWINSZ => {
                let cfg = self.config.lock();
                (arg as *mut WinSize).vm_write(cfg.winsize)?;
            }
            TIOCSWINSZ => {
                let ws: WinSize = (arg as *const WinSize).vm_read()?;
                self.config.lock().winsize = ws;
            }
            TCFLSH => {
                // arg: TCIFLUSH=0, TCOFLUSH=1, TCIOFLUSH=2
                if arg == 0 || arg == 2 {
                    self.rx_buf.lock().clear();
                }
                // Output flush is a no-op (we write synchronously)
            }
            // Silently accept these so that standard tcsetattr() sequences don't fail
            TCSBRK | TCSBRKP | TCXONC => {}
            _ => return Err(LinuxError::ENOTTY.into()),
        }
        Ok(0)
    }

    fn as_pollable(&self) -> Option<&dyn Pollable> {
        Some(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn flags(&self) -> NodeFlags {
        NodeFlags::NON_CACHEABLE | NodeFlags::STREAM
    }
}

// ─── Pollable ────────────────────────────────────────────────────────────────

impl Pollable for TtySerial {
    fn poll(&self) -> IoEvents {
        let rx = self.rx_buf.lock();
        let mut events = IoEvents::OUT; // TX is always ready (synchronous)
        if !rx.is_empty() {
            events |= IoEvents::IN;
        }
        events
    }

    fn register(&self, cx: &mut Context<'_>, events: IoEvents) {
        if events.intersects(IoEvents::IN) {
            self.poll_set.register(cx.waker());
        }
    }
}

// ─── Constructor helpers used from mod.rs ────────────────────────────────────

/// Create `/dev/ttyS1` backed by UART1 (0x04150000, IRQ 45).
/// Configures pinmux: JTAG_CPU_TMS → UART1_TX, JTAG_CPU_TCK → UART1_RX.
pub fn new_tty_s1(baud: u32) -> TtySerial {
    // Configure pinmux for UART1 before initialising the hardware
    let pinmux = Pinmux::new_with_offset(axconfig::plat::PHYS_VIRT_OFFSET);
    pinmux.set_uart1();

    TtySerial::new(UART1_PADDR, UART1_IRQ, baud, &UART1_RX_BUF, &UART1_POLL, uart1_irq_handler)
}

/// Create `/dev/ttyS2` backed by UART2 (0x04160000, IRQ 46).
/// Configures pinmux: PWR_GPIO0 → UART2_TX, PWR_GPIO1 → UART2_RX.
pub fn new_tty_s2(baud: u32) -> TtySerial {
    // Configure pinmux for UART2 before initialising the hardware
    let pinmux = Pinmux::new_with_offset(axconfig::plat::PHYS_VIRT_OFFSET);
    pinmux.set_uart2();

    TtySerial::new(UART2_PADDR, UART2_IRQ, baud, &UART2_RX_BUF, &UART2_POLL, uart2_irq_handler)
}
