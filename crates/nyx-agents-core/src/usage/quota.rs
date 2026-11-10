//! Quota checks: only for agents listed in `[usage] quotas` in
//! `~/.config/nyx-agents/config.toml`. The last good answer is kept in
//! `~/.cache/nyx-agents/usage/<agent>/quota.json`; after a failure the provider is
//! asked again only after five minutes, unless the user refreshes. Credentials are
//! read by the fetchers in this process only and never stored or sent anywhere
//! but their provider.

use nyx_agents_proto::{Pace, Quota, QuotaWindow};
use serde::{Deserialize, Serialize};

use super::time::now;

const RETRY_AFTER: f64 = 300.0;
/// Burn rates use the last six hours.
const BURN_SPAN: f64 = 6.0 * 3600.0;

/// The agents whose quota the user asked to see.
pub fn enabled(agent: &str) -> bool {
    let path = crate::paths::config_dir().join("config.toml");
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let table: toml::Table = toml::from_str(&text).unwrap_or_default();
    table
        .get("usage")
        .and_then(|u| u.get("quotas"))
        .and_then(|q| q.as_array())
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(agent)))
}

#[derive(Default, Serialize, Deserialize)]
struct Cached {
    at: f64,
    plan: Option<String>,
    windows: Vec<QuotaWindow>,
}

#[derive(Serialize, Deserialize)]
struct Failure {
    at: f64,
    error: String,
}

/// A fetcher's answer: the plan and the windows with `used_percent` and `resets_at`.
pub type Fetched = Result<(Option<String>, Vec<QuotaWindow>), String>;

/// The agent's quota: from the cache while it is younger than `max_age` seconds,
/// else from `fetch`.
pub fn get(agent: &str, refresh: bool, max_age: f64, fetch: impl FnOnce() -> Fetched) -> Quota {
    if !enabled(agent) {
        return Quota { available: true, ..Default::default() };
    }
    let dir = super::cache_dir(agent);
    let cache_path = dir.join("quota.json");
    let fail_path = dir.join("quota_failure.json");
    let read = |p: &std::path::Path| std::fs::read(p).ok();
    let cached: Option<Cached> = read(&cache_path).and_then(|b| serde_json::from_slice(&b).ok());
    let failure: Option<Failure> = read(&fail_path).and_then(|b| serde_json::from_slice(&b).ok());
    let t = now();
    let from_cache = |c: Option<Cached>, error: Option<String>| {
        let c = c.unwrap_or_default();
        let mut windows = c.windows;
        windows.iter_mut().for_each(|w| pace(w, t));
        Quota { available: true, enabled: true, plan: c.plan, windows, error, updated: (c.at > 0.0).then_some(c.at as u64) }
    };
    if !refresh {
        if let Some(c) = cached.as_ref().filter(|c| t - c.at < max_age && !c.windows.is_empty()) {
            let c = Cached { at: c.at, plan: c.plan.clone(), windows: c.windows.clone() };
            return from_cache(Some(c), None);
        }
        if let Some(f) = failure.filter(|f| t - f.at < RETRY_AFTER) {
            return from_cache(cached, Some(f.error));
        }
    }
    let _ = crate::paths::private_dir(&dir);
    match fetch() {
        Ok((plan, mut windows)) if !windows.is_empty() => {
            let _ = std::fs::remove_file(&fail_path);
            burn_rates(&dir, &mut windows, t);
            let c = Cached { at: t, plan, windows };
            if let Ok(b) = serde_json::to_vec(&c) {
                let _ = crate::paths::write_private(&cache_path, &b);
            }
            from_cache(Some(c), None)
        }
        r => {
            let error = r.err().unwrap_or_else(|| "The provider reported no limits".into());
            if let Ok(b) = serde_json::to_vec(&Failure { at: t, error: error.clone() }) {
                let _ = crate::paths::write_private(&fail_path, &b);
            }
            from_cache(cached, Some(error))
        }
    }
}

/// GET a provider's JSON, with a five-second limit. Errors name the status or the
/// failure, never the request's headers.
pub fn get_json(url: &str, headers: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(5))).http_status_as_error(false).build().into();
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut resp = req.call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        return Err("the provider didn't accept the sign-in".into());
    }
    if !(200..300).contains(&status) {
        return Err(format!("the provider answered {status}"));
    }
    let body = resp.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|_| "the provider's answer wasn't JSON".into())
}

/// Each window's use in percent per hour over the last six hours, from a snapshot
/// kept per fetch. Only the stretch since the window last refilled counts: across a
/// reset the share left jumps up, which would otherwise cancel the real burn.
/// Providers report whole percents that flicker by one, so a rise of a point or
/// less is not a refill.
fn burn_rates(dir: &std::path::Path, windows: &mut [QuotaWindow], t: f64) {
    let path = dir.join("quota_snapshots.json");
    let mut snaps: std::collections::HashMap<String, Vec<(f64, f64)>> =
        std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    for w in windows.iter_mut() {
        let left = 100.0 - w.used_percent;
        let points = snaps.entry(w.id.clone()).or_default();
        points.retain(|(at, _)| t - at < BURN_SPAN);
        points.push((t, left));
        let mut start = points.len() - 1;
        while start > 0 && points[start - 1].1 >= points[start].1 - 1.0 {
            start -= 1;
        }
        let hours = (points[points.len() - 1].0 - points[start].0) / 3600.0;
        let drop = points[start].1 - points[points.len() - 1].1;
        w.burn_per_hour = if hours >= 0.05 && drop > 0.0 { drop / hours } else { 0.0 };
    }
    snaps.retain(|id, _| windows.iter().any(|w| &w.id == id));
    if let Ok(b) = serde_json::to_vec(&snaps) {
        let _ = crate::paths::write_private(&path, &b);
    }
}

/// Where a window is heading at its burn rate.
pub fn pace(w: &mut QuotaWindow, t: f64) {
    w.left_at_reset = None;
    w.empty_in = None;
    let Some(reset) = w.resets_at else {
        w.pace = Pace::Steady;
        return;
    };
    let hours_left = (reset as f64 - t) / 3600.0;
    if hours_left <= 0.0 {
        w.pace = Pace::Stale;
        return;
    }
    if w.burn_per_hour <= 0.001 {
        w.pace = Pace::Steady;
        return;
    }
    let left = 100.0 - w.used_percent;
    let hours_to_empty = left / w.burn_per_hour;
    if hours_to_empty < hours_left {
        w.pace = Pace::RunsOut;
        w.empty_in = Some((hours_to_empty * 3600.0) as u64);
        return;
    }
    let at_reset = (left - w.burn_per_hour * hours_left).max(0.0);
    w.left_at_reset = Some(at_reset);
    w.pace = if at_reset < 15.0 { Pace::Tight } else { Pace::OnPace };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used: f64, burn: f64, hours: f64) -> QuotaWindow {
        QuotaWindow { used_percent: used, burn_per_hour: burn, resets_at: Some((now() + hours * 3600.0) as u64), ..Default::default() }
    }

    #[test]
    fn paces() {
        let t = now();
        let mut w = window(48.0, 0.5, 60.0);
        pace(&mut w, t);
        assert_eq!(w.pace, Pace::OnPace);
        assert!((w.left_at_reset.unwrap() - 22.0).abs() < 0.1);
        let mut w = window(90.0, 5.0, 4.0);
        pace(&mut w, t);
        assert_eq!(w.pace, Pace::RunsOut);
        assert_eq!(w.empty_in, Some(7200));
        let mut w = window(10.0, 0.0, 4.0);
        pace(&mut w, t);
        assert_eq!(w.pace, Pace::Steady);
        let mut w = window(10.0, 1.0, -1.0);
        pace(&mut w, t);
        assert_eq!(w.pace, Pace::Stale);
    }
}
