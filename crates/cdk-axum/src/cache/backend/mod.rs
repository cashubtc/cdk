mod memory;
mod redis;

pub use self::memory::InMemoryHttpCache;
pub use self::redis::{Config as RedisConfig, HttpCacheRedis};
