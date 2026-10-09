/// Compute source priority (0=best, 100=worst) from quality string.
pub fn compute_priority(quality: &str) -> i32 {
    match quality {
        "4k" => 10,
        "fhd" => 20,
        "hd" => 30,
        "sd" => 50,
        _ => 40, // unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_ordering() {
        assert!(compute_priority("4k") < compute_priority("fhd"));
        assert!(compute_priority("fhd") < compute_priority("hd"));
        assert!(compute_priority("hd") < compute_priority("sd"));
    }

    #[test]
    fn unknown_quality_between_hd_and_sd() {
        let unknown = compute_priority("unknown");
        assert!(unknown > compute_priority("hd"));
        assert!(unknown < compute_priority("sd"));
    }
}
