//! ID extraction functions for Kobo paths.

/// Extract book ID from a path like /v1/library/{id}/metadata.
pub fn extract_book_id(path: &str) -> Option<&str> {
    if let Some(rest) = path.strip_prefix("/v1/library/") {
        return if let Some(end) = rest.find('/') {
            Some(&rest[..end])
        } else {
            Some(rest)
        };
    }

    if let Some(rest) = path.strip_prefix("/download/")
        && let Some(end) = rest.find('/')
    {
        return Some(&rest[..end]);
    }

    // Cover image: /book-images/{id}/{w}/{h}/{g}/image.jpg
    if path.starts_with("/book-images/") && path.ends_with("/image.jpg") {
        let parts: Vec<&str> = path.split('/').collect();
        // /book-images/{id}/{w}/{h}/false/image.jpg = 7 parts
        // /book-images/{id}/{w}/{h}/{q}/{g}/image.jpg = 8 parts
        if parts.len() >= 7 {
            return Some(parts[2]); // parts[0]="", parts[1]="book-images", parts[2]=id
        }
    }

    None
}

/// Extract tag ID from a path like /v1/library/tags/{id}/items.
pub fn extract_tag_id(path: &str) -> Option<&str> {
    if let Some(rest) = path.strip_prefix("/v1/library/tags/") {
        return if let Some(end) = rest.find('/') {
            Some(&rest[..end])
        } else if !rest.is_empty() {
            Some(rest)
        } else {
            None
        };
    }

    None
}

/// Extract product ID from a path like /v1/products/{id}/nextread.
pub fn extract_product_id(path: &str) -> Option<&str> {
    if let Some(rest) = path.strip_prefix("/v1/products/") {
        return if let Some(end) = rest.find('/') {
            Some(&rest[..end])
        } else if !rest.is_empty() {
            Some(rest)
        } else {
            None
        };
    }

    None
}

/// Extract book ID from a path like /v1/products/books/{id} or /v1/products/books/{id}/.
///
/// Does NOT match /v1/products/books/series/ paths.
pub fn extract_product_book_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/v1/products/books/")?;
    // Reject series paths
    if rest.starts_with("series/") {
        return None;
    }
    let id = rest.split('/').next().unwrap_or(rest);
    let id = id.split('?').next().unwrap_or(id);
    if !id.is_empty() { Some(id) } else { None }
}

/// Extract product ID and rating value from a path like /v1/products/{id}/rating/{N}.
pub fn extract_rating_from_path(path: &str) -> Option<(&str, i32)> {
    let rest = path.strip_prefix("/v1/products/")?;
    let (product_id, after) = rest.split_once('/')?;
    let rating_str = after.strip_prefix("rating/")?;
    let rating_str = rating_str.split('/').next().unwrap_or(rating_str);
    let rating_str = rating_str.split('?').next().unwrap_or(rating_str);
    let rating: i32 = rating_str.parse().ok()?;
    if (1..=5).contains(&rating) {
        Some((product_id, rating))
    } else {
        None
    }
}

/// Extract series ID from a path like /v1/products/books/series/{id}.
pub fn extract_series_id(path: &str) -> Option<&str> {
    if let Some(rest) = path.strip_prefix("/v1/products/books/series/") {
        // Strip query parameters if present
        let id = rest.split('?').next().unwrap_or(rest);
        if !id.is_empty() {
            return Some(id);
        }
    }

    None
}
