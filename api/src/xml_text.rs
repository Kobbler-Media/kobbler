//! Reading text out of quick-xml events.
//!
//! quick-xml splits character data at every entity reference and hands the
//! reference over unresolved (`Law &amp; Order` arrives as `Law `, `amp`,
//! ` Order`), so a parser that takes one `Text` event as the whole value, or
//! trims each one, loses both the `&` and the spaces around it.

use std::borrow::Cow;

use quick_xml::XmlVersion;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::BytesRef;
use quick_xml::events::attributes::Attribute;

pub fn push_ref(buf: &mut String, reference: &BytesRef<'_>) {
    if let Ok(Some(c)) = reference.resolve_char_ref() {
        buf.push(c);
        return;
    }
    let name: &str = reference.as_ref();
    match resolve_predefined_entity(name) {
        Some(text) => buf.push_str(text),
        None => {
            buf.push('&');
            buf.push_str(name);
            buf.push(';');
        }
    }
}

pub fn attr_value(attr: &Attribute<'_>) -> String {
    attr.normalized_value(XmlVersion::Implicit1_0)
        .map(Cow::into_owned)
        .unwrap_or_else(|_| attr.value.to_string())
}
