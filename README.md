`stuck` provides a visualization for quickly identifying common
bottlenecks in **running**, **asynchronous**, and **concurrent**
applications.

It is not a replacement for other profiling tools. `stuck` is very good
at _one_ thing, and nothing else. Specifically, `stuck` shows you,
roughly, **the deepest stack frame that most threads spend most of their
time in**. Since that is the one thing `stuck` does, it's worth visiting
in a bit more detail.

Stuck samples the stack of all threads in your application periodically,
and every so often it does the following:

 1. For each thread, find the stack frame that was _most often_ on the
    thread's stack. Ties are broken by the depth of the stack frame.
 2. For each such stack frame, note down the stack up to and including
    that frame.
 3. Group the data from all the threads by that stack.
 4. Order the groups by the number of samples that were collectively
    taken for the group's stack across all threads.

It then displays these groups, along with their shared stacks and how
many threads were a part of each one. What does it look like?

[![asciicast](https://asciinema.org/a/76Q7hTZjFizMKYlHknkxEOXQH.svg)](https://asciinema.org/a/76Q7hTZjFizMKYlHknkxEOXQH)

## How do you run it?

First, install `stuck`:
```console
$ cargo install stuck
```

Then, run a program of some sort that you want to benchmark.

Then, use [`bpftrace`](https://github.com/iovisor/bpftrace/) to profile
that application. A low sampling rate (`hz:1`) is probably fine, but
adjust as you see fit (e.g., `hz:99`). Pipe the result into `stuck`.

```console
$ pid=$(pgrep my_program)
$ sudo env BPFTRACE_NO_CPP_DEMANGLE=1 bpftrace -e 'profile:hz:1 /pid == '"$pid"'/ { printf("%ld %d %s\n", elapsed, tid, ustack) }' | stuck
```

That's it!

## Profiling Rust programs

First, you _probably_ want to enable debug symbols in release mode, so
that you don't just end up with unhelpful addresses in your output.

```toml
[profile.release]
debug=true
```

Second, `bpftrace`
[requires](https://github.com/iovisor/bpftrace/issues/1006) that your
program is compiled with frame pointers. Rust does not do so by default,
so you have to set a Rust compiler flag to make it work correctly:

```console
$ env RUSTFLAGS='-Cforce-frame-pointers=yes' cargo build --release
```

(If you're a gcc/clang person, use `-fno-omit-frame-pointer`)

## Development status

Very much experimental. Broken in several known ways:

 - No way to quit except by having the input reach EOF.
 - [Demangling does not work](https://github.com/alexcrichton/rustc-demangle/issues/34).
 - Running `stuck` from a trace file is useless (it needs to `sleep`).
