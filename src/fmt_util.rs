use anyhow::{bail, Result};

const UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];

/// Human-readable size, e.g. "1.4 GB" (decimal units, like most disk tools).
pub fn human_size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{} B", bytes);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else if value >= 10.0 {
        format!("{:.1} {}", value, UNITS[unit])
    } else {
        format!("{:.2} {}", value, UNITS[unit])
    }
}

/// Parse "500", "10KB", "1.5GB", "2 gb" into bytes (decimal units).
pub fn parse_size(input: &str) -> Result<u64> {
    let s = input.trim().to_ascii_uppercase();
    let split = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: f64 = match num.trim().parse() {
        Ok(v) => v,
        Err(_) => bail!("invalid size: {input}"),
    };
    let mult: f64 = match unit.trim() {
        "" | "B" => 1.0,
        "K" | "KB" => 1e3,
        "M" | "MB" => 1e6,
        "G" | "GB" => 1e9,
        "T" | "TB" => 1e12,
        _ => bail!("invalid size unit: {input}"),
    };
    Ok((value * mult) as u64)
}

/// A proportional bar like "▕████      ▏" (dust-style: solid fill, blank rest).
pub fn bar(fraction: f64, width: usize) -> String {
    let filled = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let mut s = String::with_capacity(width * 3 + 6);
    s.push('▕');
    for i in 0..width {
        s.push(if i < filled { '█' } else { ' ' });
    }
    s.push('▏');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1_000), "1.00 KB");
        assert_eq!(human_size(1_500_000), "1.50 MB");
        assert_eq!(human_size(123_456_789_000), "123 GB");
    }

    #[test]
    fn parse_sizes() {
        assert_eq!(parse_size("500").unwrap(), 500);
        assert_eq!(parse_size("10KB").unwrap(), 10_000);
        assert_eq!(parse_size("1.5GB").unwrap(), 1_500_000_000);
        assert_eq!(parse_size("2 mb").unwrap(), 2_000_000);
        assert!(parse_size("abc").is_err());
    }
}
