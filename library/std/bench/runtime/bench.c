/*
 * std::bench runtime — opaque-identity black_box only.
 *
 * The whole bench harness lives in pure Ruxen (`src/lib.rx`); the
 * only thing C is doing here is preventing the compiler from
 * inlining + constant-folding the value the user wants to keep
 * "live" through the bench iteration. A pure-Ruxen `def black_box[T](
 * x: T) -> T; x; end` would be optimised away because the compiler
 * sees the body is a pass-through.
 *
 * The `__attribute__((noinline))` keeps LLVM/Cranelift from inlining
 * the call site; the `asm volatile` reads the value through a no-op
 * inline-asm constraint so the value is genuinely observed at this
 * point and the producer code cannot be dropped. Same trick that
 * `std::hint::black_box` in Rust uses.
 *
 * Ruxen-side declaration: `def self.black_box as "ruxen_bench_black_box"(v: Int) -> Int`.
 * `Int` (i64) is the only typed surface — closure return values that
 * are heap-typed (String, Array, ...) are already opaque to the
 * optimizer through the pointer indirection, so the Int specialisation
 * is enough for v1. A generic-over-T black_box is post-v1.
 */
#include <stdint.h>

__attribute__((noinline))
int64_t ruxen_bench_black_box(int64_t v) {
    __asm__ __volatile__ ("" : "+r"(v) : : "memory");
    return v;
}
