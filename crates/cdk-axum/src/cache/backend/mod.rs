mod memory;
#[cfg(feature = "redis")]
mod redis;

pub use self::memory::InMemoryHttpCache;
#[cfg(feature = "redis")]
pub use self::redis::{Config as RedisConfig, HttpCacheRedis};
