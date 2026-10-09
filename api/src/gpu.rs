//! GPU coordinator — promoted to the shared `kob-gpu-gate` crate so koblem
//! and kobbler share one implementation. This module just re-exports the
//! types so existing `crate::gpu::GpuLock` call sites keep working.

pub use kob_gpu_gate::{GpuLock, GpuSttGuard, GpuTtsGuard, GpuVisionGuard, ServiceKind};
