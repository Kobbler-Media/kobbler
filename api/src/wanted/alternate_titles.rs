use std::collections::HashSet;

fn without_article(title: &str) -> Option<&str> {
    let rest = title
        .get(..4)
        .filter(|prefix| prefix.eq_ignore_ascii_case("the "))
        .map(|_| title[4..].trim())?;
    (!rest.is_empty()).then_some(rest)
}

/// Release groups drop the article freely ("Office.S01E01").
pub fn alternate_titles<S: AsRef<str>>(
    primary: &str,
    known: impl IntoIterator<Item = S>,
) -> Option<Vec<String>> {
    let mut seen: HashSet<String> = HashSet::from([primary.trim().to_lowercase()]);
    let mut out = Vec::new();
    let mut push = |title: String| {
        if !title.is_empty() && seen.insert(title.to_lowercase()) {
            out.push(title);
        }
    };
    let known: Vec<String> = known
        .into_iter()
        .map(|t| t.as_ref().trim().to_string())
        .collect();
    for title in &known {
        push(title.clone());
    }
    let primary = primary.trim();
    match without_article(primary) {
        Some(bare) => push(bare.to_string()),
        None if !primary.is_empty() => push(format!("The {primary}")),
        None => {}
    }
    for title in &known {
        if let Some(bare) = without_article(title) {
            push(bare.to_string());
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_alternates_come_first_then_article_variants() {
        assert_eq!(
            alternate_titles("The Thing", ["La Cosa"]),
            Some(vec!["La Cosa".to_string(), "Thing".to_string()])
        );
        assert_eq!(
            alternate_titles("Office", Vec::<String>::new()),
            Some(vec!["The Office".to_string()])
        );
    }

    #[test]
    fn the_primary_title_and_duplicates_are_left_out() {
        assert_eq!(
            alternate_titles("The Office", ["the office", "Office", " Office "]),
            Some(vec!["Office".to_string()])
        );
        assert_eq!(alternate_titles("", Vec::<String>::new()), None);
        assert_eq!(
            alternate_titles("Thing", ["The Thing From Another World"]),
            Some(vec![
                "The Thing From Another World".to_string(),
                "The Thing".to_string(),
                "Thing From Another World".to_string()
            ])
        );
    }
}
