# AdityaOS

A small bare-metal operating system for **AArch64 (ARM64)**, written from scratch in **Rust and assembly**.

AdityaOS boots directly on QEMU's `virt` machine, without Linux or another operating system underneath. It currently includes a preemptive multitasking kernel, interrupt-driven scheduling, an interactive shell, UART I/O, and basic hardware inspection tools.

Built by **Aditya** from Visakhapatnam, India.

---

## Features

- Bare-metal AArch64 (ARM64) kernel
- Custom assembly boot code
- Custom linker script and memory layout
- PL011 UART driver for terminal input/output
- `print!` and `println!` macros through `core::fmt::Write`
- AArch64 exception vector table
- Synchronous exception and IRQ handling
- GICv2 interrupt controller support
- ARM Generic Timer configuration
- Preemptive multitasking
- Round-robin thread scheduler
- Runtime thread spawning
- Interactive kernel shell
- Command history
- Memory inspection
- CPU register/state inspection
- Kernel panic handler with fault-address reporting
- PSCI-based system reset

---

## Shell

AdityaOS includes an interactive shell running as a kernel thread.

| Command | Description |
|---|---|
| `help` | List available commands |
| `clear` | Clear the terminal |
| `ticks` | Show the timer tick count |
| `uptime` / `time` | Show seconds since boot |
| `threads` | Show thread count, current thread, and context-switch count |
| `el` | Show the current exception level |
| `regs` | Dump CPU state including SP, ELR, SPSR, and CurrentEL |
| `echo <text>` | Print text to the terminal |
| `sleep <ms>` | Sleep for the specified number of milliseconds |
| `mem <addr>` | Dump 128 bytes starting at a hexadecimal address |
| `history` | Show recent commands |
| `spawn` | Create a new kernel thread at runtime |
| `panic` | Trigger a kernel panic for testing |
| `reboot` | Reset the machine through PSCI |

---

## Requirements

- Rust nightly
- `aarch64-unknown-none` Rust target
- QEMU 8.x or newer
- `aarch64-linux-gnu-gcc`
- AArch64 binutils

### Ubuntu / WSL2

Install the required packages:

```bash
sudo apt update
sudo apt install build-essential qemu-system-arm gcc-aarch64-linux-gnu binutils-aarch64-linux-gnu
```

Install Rust using `rustup` if it is not already installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then install the AArch64 bare-metal target:

```bash
rustup target add aarch64-unknown-none
```

---

## Build and Run

Clone the repository and run:

```bash
./build.sh
```

The build script compiles the kernel and launches it in QEMU.

To exit QEMU:

```text
Ctrl + A
```

Release the keys, then press:

```text
X
```

---

## Project Layout

```text
AdityaOS/
├── .cargo/
│   └── config.toml       # Cargo target and linker configuration
├── build.rs              # Assembles boot.S and configures the build
├── linker.ld             # Kernel memory layout
├── build.sh              # Build and run script
├── LICENSE               # MIT License
└── src/
    ├── boot.S            # AArch64 entry point, vectors, and IRQ handling
    └── main.rs           # Kernel, UART, GIC, timer, scheduler, and shell
```

---

## How It Works

### 1. Boot

QEMU loads the kernel at `0x40080000` and transfers execution to `_start` in `boot.S`.

The assembly boot code:

- Sets up the initial stack
- Clears the BSS section
- Installs the exception vector table
- Configures `VBAR_EL1`
- Transfers control to the Rust kernel

### 2. Kernel Initialization

Rust's `kmain` initializes the core hardware:

- PL011 UART
- GICv2 interrupt controller
- ARM Generic Timer
- Exception handling

The timer is configured to generate periodic interrupts, allowing the kernel to perform scheduling.

### 3. Preemptive Scheduler

AdityaOS uses a **round-robin scheduler**.

Timer IRQs periodically invoke the scheduler. When a context switch occurs, the current CPU state is saved into the thread's `Context` structure and the next runnable thread's context is restored.

This allows multiple kernel threads to execute without each thread having to voluntarily yield.

### 4. Shell

The shell runs as a kernel thread and communicates directly with the PL011 UART.

Input is read from the UART receive FIFO, parsed into commands, and dispatched to the corresponding kernel functionality.

---

## Architecture

```text
                    QEMU virt
                       │
                       ▼
                ┌──────────────┐
                │    boot.S    │
                │ AArch64 boot │
                └──────┬───────┘
                       │
                       ▼
                ┌──────────────┐
                │    Kernel    │
                │    Rust      │
                └──────┬───────┘
                       │
          ┌────────────┼────────────┐
          ▼            ▼            ▼
       PL011          GICv2      Generic Timer
        UART          IRQs          │
          │            │            │
          └────────────┴─────┬──────┘
                             ▼
                    ┌────────────────┐
                    │  Scheduler     │
                    │ Round-robin    │
                    └───────┬────────┘
                            │
                 ┌──────────┼──────────┐
                 ▼          ▼          ▼
              Shell      Thread 1   Thread 2
```

---

## Known Limitations

AdityaOS is intentionally small and is still an experimental kernel.

- No MMU or virtual memory yet
- No user mode; kernel code currently runs at EL1
- No process isolation
- Threads cannot currently exit or be joined
- Spawned threads currently run indefinitely
- UART input is polling-based rather than interrupt-driven
- No filesystem
- No persistent storage
- No networking
- No SMP/multicore support yet

---

## Roadmap

Possible future improvements include:

- [ ] MMU and virtual memory
- [ ] EL0 user-mode programs
- [ ] System calls
- [ ] Thread termination and joining
- [ ] Interrupt-driven UART input
- [ ] Heap allocator
- [ ] Filesystem
- [ ] SMP / multicore support
- [ ] User-space shell
- [ ] More hardware drivers

---

## License

AdityaOS is released under the **MIT License**.

See [`LICENSE`](LICENSE) for the full license text.
