use super::{events, LocationSettings, Position};
use futures::{Stream, StreamExt};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationPermission {
    NotDetermined,
    Denied,
    Restricted,
    Approximate,
    Precise,
}
#[derive(Debug, Clone)]
pub enum LocationEvent {
    Permission(LocationPermission),
    Position(Position),
    Error {
        code: String,
        message: String,
    },
    /// The native owner ended the subscription; the next item is `None`.
    Ended {
        reason: String,
    },
}
/// Foreground subscription; drop or cancel stops the native listener.
/// Native callbacks run independently of the executor polling this stream.
pub struct LocationSubscription {
    receiver: events::Receiver,
    settings: LocationSettings,
    stopped: bool,
}
impl LocationSubscription {
    pub fn cancel(&mut self) {
        if !self.stopped {
            self.receiver.close();
            stop(self.receiver.id);
            self.stopped = true;
        }
    }
    /// Retry after returning from Settings / after foreground resume.
    /// Permission prompts remain an explicit application action on Android.
    pub fn refresh(&self) -> Result<(), String> {
        if self.stopped || self.receiver.is_closed() {
            return Err("Subscription cancelled".into());
        }
        start(self.receiver.id, &self.settings, false)
    }
}
impl Drop for LocationSubscription {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Stream for LocationSubscription {
    type Item = LocationEvent;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.receiver).poll_next(cx) {
            Poll::Ready(Some(raw)) => Poll::Ready(Some(parse_event(&raw))),
            Poll::Ready(None) => {
                self.cancel();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
pub fn watch_position(settings: &LocationSettings) -> Result<LocationSubscription, String> {
    subscribe(settings, false)
}
fn subscribe(settings: &LocationSettings, once: bool) -> Result<LocationSubscription, String> {
    if !settings.distance_filter.is_finite()
        || settings.distance_filter < 0.0
        || settings.distance_filter > f32::MAX as f64
    {
        return Err("distance_filter must be finite, nonnegative and representable as f32".into());
    }
    let subscription = LocationSubscription {
        receiver: events::Receiver::new(),
        settings: settings.clone(),
        stopped: false,
    };
    start(subscription.receiver.id, settings, once)?;
    Ok(subscription)
}
/// One fresh fix; native backends enforce a 30-second timeout. Dropping this
/// future cancels its listener, including while waiting for authorization.
pub async fn get_current_position_async(settings: &LocationSettings) -> Result<Position, String> {
    first_position(subscribe(settings, true)?).await
}
async fn first_position(mut subscription: LocationSubscription) -> Result<Position, String> {
    while let Some(event) = subscription.next().await {
        match event {
            LocationEvent::Position(position) => return Ok(position),
            LocationEvent::Error { code, message } => return Err(format!("{code}: {message}")),
            LocationEvent::Ended { reason } => {
                return Err(format!("Location subscription ended: {reason}"))
            }
            LocationEvent::Permission(
                LocationPermission::Denied | LocationPermission::Restricted,
            ) => return Err("Location permission denied".into()),
            _ => {}
        }
    }
    Err("Location subscription ended".into())
}
fn start(id: u64, settings: &LocationSettings, once: bool) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        return super::ios::start(id, settings, once);
    }
    #[cfg(target_os = "android")]
    {
        return super::android::start(id, settings, once);
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = (id, settings, once);
        Err("Location not supported on this platform".into())
    }
}
fn stop(id: u64) {
    #[cfg(target_os = "ios")]
    super::ios::stop(id);
    #[cfg(target_os = "android")]
    super::android::stop(id);
    let _ = id;
}
fn parse_event(raw: &str) -> LocationEvent {
    let mut fields = raw.splitn(3, '|');
    let kind = fields.next().unwrap_or("");
    let value = fields.next().unwrap_or("");
    match kind {
        "permission" => match value {
            "not_determined" => LocationEvent::Permission(LocationPermission::NotDetermined),
            "denied" => LocationEvent::Permission(LocationPermission::Denied),
            "restricted" => LocationEvent::Permission(LocationPermission::Restricted),
            "approximate" => LocationEvent::Permission(LocationPermission::Approximate),
            "precise" => LocationEvent::Permission(LocationPermission::Precise),
            _ => LocationEvent::Error {
                code: "protocol".into(),
                message: raw.into(),
            },
        },
        "position" => match parse_position(raw.strip_prefix("position|").unwrap_or("")) {
            Ok(position) => LocationEvent::Position(position),
            Err(message) => LocationEvent::Error {
                code: "protocol".into(),
                message,
            },
        },
        "end" => LocationEvent::Ended {
            reason: value.into(),
        },
        "error" => LocationEvent::Error {
            code: value.into(),
            message: fields.next().unwrap_or("").into(),
        },
        _ => LocationEvent::Error {
            code: "protocol".into(),
            message: raw.into(),
        },
    }
}
pub(super) fn parse_position(raw: &str) -> Result<Position, String> {
    let values: Vec<&str> = raw.split('|').collect();
    if values.len() != 9 {
        return Err("Expected nine position fields".into());
    }
    let number = |i: usize| -> Result<f64, String> {
        let value = values[i]
            .parse::<f64>()
            .map_err(|_| "Invalid position number".to_string())?;
        if value.is_infinite() {
            return Err("Infinite position number".into());
        }
        Ok(value)
    };
    let latitude = number(0)?;
    let longitude = number(1)?;
    let accuracy = number(3)?;
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !accuracy.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
        || accuracy < 0.0
    {
        return Err("Invalid coordinates or horizontal accuracy".into());
    }
    let timestamp = values[8]
        .parse::<u64>()
        .map_err(|_| "Invalid timestamp".to_string())?;
    Ok(Position {
        latitude,
        longitude,
        altitude: number(2)?,
        accuracy,
        speed: number(4)?,
        speed_accuracy: number(5)?,
        heading: number(6)?,
        heading_accuracy: number(7)?,
        timestamp,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    fn test_subscription() -> LocationSubscription {
        LocationSubscription {
            receiver: events::Receiver::new(),
            settings: LocationSettings::default(),
            stopped: false,
        }
    }
    #[test]
    fn native_owner_end_completes_one_shot_without_waiting_for_timeout() {
        let subscription = test_subscription();
        events::send(subscription.receiver.id, "end|owner_destroyed".into());
        assert!(first_position(subscription)
            .now_or_never()
            .expect("must terminate without a timer")
            .unwrap_err()
            .contains("owner_destroyed"));
    }
    #[test]
    fn terminal_configuration_error_ends_tracking() {
        let mut subscription = test_subscription();
        events::send(
            subscription.receiver.id,
            "error|configuration|missing usage description".into(),
        );
        events::send(subscription.receiver.id, "end|configuration".into());
        assert!(matches!(
            futures::executor::block_on(subscription.next()),
            Some(LocationEvent::Error { .. })
        ));
        assert!(matches!(
            futures::executor::block_on(subscription.next()),
            Some(LocationEvent::Ended { .. })
        ));
        assert!(futures::executor::block_on(subscription.next()).is_none());
        assert!(subscription.refresh().is_err());
    }
    #[test]
    fn malformed_samples_are_errors() {
        assert!(matches!(
            parse_event("position"),
            LocationEvent::Error { .. }
        ));
        for raw in [
            "NaN|1|0|1|0|0|0|0|1",
            "91|1|0|1|0|0|0|0|1",
            "1|1|0|-1|0|0|0|0|1",
            "1|1|0|1|0|0|0|0|-1",
        ] {
            assert!(parse_position(raw).is_err());
        }
    }
    #[test]
    fn preserves_unknown_optional_metadata() {
        let p = parse_position("37|127|NaN|5|NaN|NaN|NaN|NaN|1700000000001").unwrap();
        assert!(p.speed.is_nan());
        assert_eq!(p.timestamp, 1700000000001);
    }
    #[test]
    fn permission_recovery_parses_and_continues() {
        assert!(matches!(
            parse_event("permission|denied"),
            LocationEvent::Permission(LocationPermission::Denied)
        ));
        assert!(matches!(
            parse_event("permission|approximate"),
            LocationEvent::Permission(LocationPermission::Approximate)
        ));
    }
}
