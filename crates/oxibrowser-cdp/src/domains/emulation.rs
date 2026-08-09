//! CDP Emulation domain handler.
//!
//! Handles `Emulation.setDeviceMetricsOverride`, `Emulation.clearDeviceMetricsOverride`,
//! `Emulation.setVisibleSize`, and `Emulation.setUserAgentOverride`.
//!
//! Stored metrics are kept in a module-level static so render code can read them
//! later via [`current_device_metrics`] without round-tripping through the protocol.

use crate::domains::DomainResult;
use crate::protocol::CdpError;
use serde_json::{Value, json};
use std::sync::LazyLock;

/// Stored device metrics override (set via `Emulation.setDeviceMetricsOverride`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceMetrics {
    /// Override width in CSS pixels.
    pub width: u32,
    /// Override height in CSS pixels.
    pub height: u32,
    /// Override device scale factor.
    pub device_scale_factor: f64,
    /// Whether to emulate a mobile device.
    pub mobile: bool,
}

static DEVICE_METRICS: LazyLock<parking_lot::RwLock<Option<DeviceMetrics>>> =
    LazyLock::new(|| parking_lot::RwLock::new(None));

/// Read the currently stored device metrics override, if any.
///
/// Returns `None` after `Emulation.clearDeviceMetricsOverride` (or before any
/// `Emulation.setDeviceMetricsOverride` call).
pub fn current_device_metrics() -> Option<DeviceMetrics> {
    *DEVICE_METRICS.read()
}

/// Dispatch Emulation domain methods.
pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "setDeviceMetricsOverride" => set_device_metrics_override(params),
        "clearDeviceMetricsOverride" => clear_device_metrics_override(),
        "setVisibleSize" => Ok(Some(json!({}))),
        "setUserAgentOverride" => Ok(Some(json!({}))),
        _ => Err(CdpError {
            code: -32601,
            message: format!("Emulation.{method} not implemented"),
        }),
    }
}

/// `Emulation.setDeviceMetricsOverride` — store viewport + scale + mobile flag.
///
/// Missing fields default to `width=1280`, `height=800`, `deviceScaleFactor=1.0`,
/// `mobile=false`. `width`/`height` are clamped to `>= 1`.
fn set_device_metrics_override(params: Option<Value>) -> DomainResult {
    let params = params.unwrap_or_default();
    let width = params
        .get("width")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(1280)
        .max(1);
    let height = params
        .get("height")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(800)
        .max(1);
    let device_scale_factor = params
        .get("deviceScaleFactor")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    let mobile = params
        .get("mobile")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    tracing::debug!(
        width,
        height,
        device_scale_factor,
        mobile,
        "Emulation.setDeviceMetricsOverride"
    );

    *DEVICE_METRICS.write() = Some(DeviceMetrics {
        width,
        height,
        device_scale_factor,
        mobile,
    });

    Ok(Some(json!({})))
}

/// `Emulation.clearDeviceMetricsOverride` — drop any stored override.
fn clear_device_metrics_override() -> DomainResult {
    *DEVICE_METRICS.write() = None;
    tracing::debug!("Emulation.clearDeviceMetricsOverride");
    Ok(Some(json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // The device metrics override lives in a process-global static, so tests
    // that touch it must run serially to avoid cross-contamination when cargo
    // runs them on multiple threads.
    static TEST_LOCK: LazyLock<parking_lot::Mutex<()>> =
        LazyLock::new(|| parking_lot::Mutex::new(()));

    /// Acquire the serial-test guard. RAII — released on drop.
    fn serial() -> parking_lot::MutexGuard<'static, ()> {
        TEST_LOCK.lock()
    }

    #[test]
    fn set_device_metrics_override_stores_values() {
        let _g = serial();
        *DEVICE_METRICS.write() = None;

        let params = json!({
            "width": 375u32,
            "height": 812u32,
            "deviceScaleFactor": 3.0,
            "mobile": true,
        });
        let result = handle("setDeviceMetricsOverride", Some(params)).unwrap();
        assert_eq!(result, Some(json!({})));

        let stored = current_device_metrics().expect("metrics should be set");
        assert_eq!(stored.width, 375);
        assert_eq!(stored.height, 812);
        assert_eq!(stored.device_scale_factor, 3.0);
        assert!(stored.mobile);
    }

    #[test]
    fn clear_device_metrics_override_returns_empty_and_clears_state() {
        let _g = serial();
        *DEVICE_METRICS.write() = Some(DeviceMetrics {
            width: 100,
            height: 200,
            device_scale_factor: 1.0,
            mobile: false,
        });

        let result = handle("clearDeviceMetricsOverride", None).unwrap();
        assert_eq!(result, Some(json!({})));

        assert!(current_device_metrics().is_none());
    }

    #[test]
    fn unknown_method_returns_method_not_implemented() {
        let result = handle("setCPUThrottlingRate", None);
        let err = result.expect_err("expected error");
        assert_eq!(err.code, -32601);
        assert!(
            err.message.contains("Emulation.setCPUThrottlingRate"),
            "message should name the unknown method: {}",
            err.message
        );
    }

    #[test]
    fn set_visible_size_acknowledges() {
        let result = handle("setVisibleSize", None).unwrap();
        assert_eq!(result, Some(json!({})));
    }

    #[test]
    fn set_user_agent_override_acknowledges() {
        let params = json!({ "userAgent": "Mozilla/5.0" });
        let result = handle("setUserAgentOverride", Some(params)).unwrap();
        assert_eq!(result, Some(json!({})));
    }

    #[test]
    fn set_device_metrics_override_clamps_zero_dimensions() {
        let _g = serial();
        *DEVICE_METRICS.write() = None;

        let params = json!({
            "width": 0u32,
            "height": 0u32,
        });
        handle("setDeviceMetricsOverride", Some(params)).unwrap();

        let stored = current_device_metrics().expect("metrics should be set");
        assert!(stored.width >= 1);
        assert!(stored.height >= 1);
    }

    #[test]
    fn set_device_metrics_override_applies_defaults_when_params_missing() {
        let _g = serial();
        *DEVICE_METRICS.write() = None;

        handle("setDeviceMetricsOverride", None).unwrap();
        let stored = current_device_metrics().expect("metrics should be set");
        assert_eq!(stored.width, 1280);
        assert_eq!(stored.height, 800);
        assert_eq!(stored.device_scale_factor, 1.0);
        assert!(!stored.mobile);
    }
}
