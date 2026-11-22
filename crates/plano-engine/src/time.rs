//! Conversions de dates sans dépendance externe (calendrier grégorien, UTC + décalage fixe).

use plano_core::Ts;

const DAY: i64 = 86_400;

/// Jours écoulés depuis le 1er janvier 1970 pour une date civile.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Date civile (année, mois, jour) pour un nombre de jours depuis 1970.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Lit « HH:MM ».
pub fn parse_hhmm(s: &str) -> Option<(u8, u8)> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u8 = h.parse().ok()?;
    let m: u8 = m.parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

/// Lit « AAAA-MM-JJ HH:MM » exprimé en heure locale (décalage `utc_offset_s`).
pub fn parse_local(s: &str, utc_offset_s: i64) -> Option<Ts> {
    let (date, time) = s.trim().split_once(' ')?;
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let (h, mi) = parse_hhmm(time)?;
    let days = days_from_civil(y, m, d);
    // Refuse les dates impossibles (31 février, mois 13…).
    if civil_from_days(days) != (y, m, d) {
        return None;
    }
    Some(days * DAY + i64::from(h) * 3_600 + i64::from(mi) * 60 - utc_offset_s)
}

/// Écrit « AAAA-MM-JJ HH:MM » en heure locale.
pub fn format_local(ts: Ts, utc_offset_s: i64) -> String {
    let local = ts + utc_offset_s;
    let days = local.div_euclid(DAY);
    let secs = local.rem_euclid(DAY);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3_600,
        (secs % 3_600) / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoque_et_date_connue() {
        assert_eq!(parse_local("1970-01-01 00:00", 0), Some(0));
        assert_eq!(parse_local("2000-03-01 00:00", 0), Some(951_868_800));
    }

    #[test]
    fn decalage_horaire() {
        assert_eq!(parse_local("1970-01-01 01:00", 3_600), Some(0));
        assert_eq!(format_local(0, 3_600), "1970-01-01 01:00");
    }

    #[test]
    fn aller_retour() {
        let s = "2026-11-25 18:00";
        let ts = parse_local(s, 3_600).unwrap();
        assert_eq!(format_local(ts, 3_600), s);
    }

    #[test]
    fn dates_invalides() {
        assert_eq!(parse_local("2026-02-31 10:00", 0), None);
        assert_eq!(parse_local("2026-13-01 10:00", 0), None);
        assert_eq!(parse_local("2026-11-25", 0), None);
        assert_eq!(parse_local("2026-11-25 25:00", 0), None);
    }

    #[test]
    fn heure_minute() {
        assert_eq!(parse_hhmm("08:05"), Some((8, 5)));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("8h"), None);
    }
}
