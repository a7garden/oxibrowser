//! Network layer — HTTP client, cookie jar, resource loading, IP filtering, robots.txt.

pub mod client;
pub mod cookie;
pub mod ip_filter;
pub mod resource;
pub mod robots;

pub use client::HttpClient;
pub use cookie::CookieJar;
pub use ip_filter::IpFilter;
pub use robots::RobotStore;
