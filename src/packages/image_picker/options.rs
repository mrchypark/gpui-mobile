pub(super) fn validate(
    width: Option<f64>,
    height: Option<f64>,
    quality: Option<u8>,
) -> Result<(), String> {
    for value in [width, height].into_iter().flatten() {
        if !value.is_finite() || value < 1.0 || value > i32::MAX as f64 {
            return Err(
                "Image bounds must be finite and at least one pixel, at most i32::MAX".into(),
            );
        }
    }
    if quality.is_some_and(|q| q > 100) {
        return Err("JPEG quality must be 0..100".into());
    }
    Ok(())
}

#[cfg(any(target_os = "ios", test))]
pub(super) fn fit(
    width: u32,
    height: u32,
    max_width: Option<f64>,
    max_height: Option<f64>,
) -> Result<(u32, u32), String> {
    validate(max_width, max_height, None)?;
    if width == 0 || height == 0 {
        return Err("Image has invalid dimensions".into());
    }
    let scale = 1.0f64
        .min(max_width.unwrap_or(width as f64) / width as f64)
        .min(max_height.unwrap_or(height as f64) / height as f64);
    Ok((
        (width as f64 * scale).floor().max(1.0) as u32,
        (height as f64 * scale).floor().max(1.0) as u32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_exposed_options_before_presentation() {
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY, 0.5] {
            assert!(validate(Some(value), None, None).is_err());
        }
        assert!(validate(None, None, Some(101)).is_err());
        assert!(validate(Some(1.0), None, Some(0)).is_ok());
    }
    #[test]
    fn respects_both_bounds_without_upscaling() {
        assert_eq!(
            fit(4000, 3000, Some(1000.0), Some(600.0)).unwrap(),
            (800, 600)
        );
        assert_eq!(
            fit(3000, 4000, Some(600.0), Some(1000.0)).unwrap(),
            (600, 800)
        );
        assert_eq!(fit(40, 20, Some(1000.0), None).unwrap(), (40, 20));
        assert_eq!(fit(4000, 1, Some(1.0), None).unwrap(), (1, 1));
    }
}
