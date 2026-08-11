//! A string that is cheap to clone.
//!
//! Covers the three shapes a short, frequently cloned string takes: a `'static`
//! literal, a borrow from a caller-owned buffer, and a refcounted owned string.
//! All of them compare, order and hash purely by content, so the variant a value
//! happens to be in is never observable.
//!
//! Deserializing always allocates, which keeps `CheapStr<'static>` usable as a
//! field of a `#[derive(Deserialize)]` type. [`deserialize_borrowed`] is the
//! opt-in zero-copy path.

use std::borrow::{Borrow, Cow};
use std::cmp::Ordering;
use std::convert::Infallible;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use serde::de::{Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// A string that is cheap to clone.
///
/// Cloning is free for the `'static` and borrowed cases and a refcount bump for
/// the owned case.
#[derive(Default, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub struct CheapStr<'a>(Inner<'a>);

#[derive(Eq, Clone)]
enum Inner<'a> {
    Static(&'static str),
    Borrowed(&'a str),
    Owned(Arc<str>),
}

impl Inner<'_> {
    #[inline]
    fn as_str(&self) -> &str {
        match self {
            Self::Static(s) => s,
            Self::Borrowed(s) => s,
            Self::Owned(s) => s,
        }
    }
}

// The derived impls would key off the variant discriminant, making
// `Static("x") != Borrowed("x")` and breaking `Borrow<str>` lookups.

impl Default for Inner<'_> {
    fn default() -> Self {
        Self::Static("")
    }
}

impl PartialEq for Inner<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialOrd for Inner<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Inner<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for Inner<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl CheapStr<'static> {
    /// Wraps a string literal without allocating.
    pub const fn from_static(value: &'static str) -> Self {
        Self(Inner::Static(value))
    }
}

impl<'a> CheapStr<'a> {
    /// Borrows an existing string without allocating.
    pub const fn from_borrowed(value: &'a str) -> Self {
        Self(Inner::Borrowed(value))
    }

    /// The string contents.
    #[inline]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Whether the contents are empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.as_str().is_empty()
    }

    /// Length of the contents in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.as_str().len()
    }

    /// Detaches from `'a`, allocating only if the value is currently borrowed.
    pub fn into_static(self) -> CheapStr<'static> {
        CheapStr(match self.0 {
            Inner::Static(s) => Inner::Static(s),
            Inner::Borrowed(s) => Inner::Owned(s.into()),
            Inner::Owned(s) => Inner::Owned(s),
        })
    }
}

impl fmt::Debug for CheapStr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for CheapStr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl Deref for CheapStr<'_> {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for CheapStr<'_> {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for CheapStr<'_> {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl<'a> From<&'a str> for CheapStr<'a> {
    fn from(value: &'a str) -> Self {
        Self(Inner::Borrowed(value))
    }
}

impl From<String> for CheapStr<'_> {
    fn from(value: String) -> Self {
        Self(Inner::Owned(value.into()))
    }
}

impl From<Arc<str>> for CheapStr<'_> {
    fn from(value: Arc<str>) -> Self {
        Self(Inner::Owned(value))
    }
}

impl<'a> From<Cow<'a, str>> for CheapStr<'a> {
    fn from(value: Cow<'a, str>) -> Self {
        match value {
            Cow::Borrowed(s) => Self(Inner::Borrowed(s)),
            Cow::Owned(s) => Self(Inner::Owned(s.into())),
        }
    }
}

impl From<CheapStr<'_>> for String {
    fn from(value: CheapStr<'_>) -> Self {
        value.as_str().to_owned()
    }
}

impl FromStr for CheapStr<'_> {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(Inner::Owned(value.into())))
    }
}

impl PartialEq<str> for CheapStr<'_> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for CheapStr<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for CheapStr<'_> {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<CheapStr<'_>> for str {
    fn eq(&self, other: &CheapStr<'_>) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<CheapStr<'_>> for String {
    fn eq(&self, other: &CheapStr<'_>) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Serialize for CheapStr<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct OwningVisitor<'a>(PhantomData<&'a ()>);

impl<'de, 'a> Visitor<'de> for OwningVisitor<'a> {
    type Value = CheapStr<'a>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(CheapStr(Inner::Owned(value.into())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(CheapStr(Inner::Owned(value.into())))
    }
}

// Deliberately no `'de: 'a` bound: it would exclude `'a = 'static` for every
// real deserializer, and `CheapStr<'static>` is the form callers store.
// Borrowing is opt-in through `deserialize_borrowed` instead, mirroring how
// serde treats `Cow<'a, str>`.
impl<'de, 'a> Deserialize<'de> for CheapStr<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(OwningVisitor(PhantomData))
    }
}

struct BorrowingVisitor<'a>(PhantomData<&'a ()>);

impl<'de: 'a, 'a> Visitor<'de> for BorrowingVisitor<'a> {
    type Value = CheapStr<'a>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string")
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
        Ok(CheapStr(Inner::Borrowed(value)))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(CheapStr(Inner::Owned(value.into())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(CheapStr(Inner::Owned(value.into())))
    }
}

/// Deserializes without allocating when the input can lend the string.
///
/// The [`Deserialize`] impl always allocates so that `CheapStr<'static>` stays
/// usable. Fields that can tie themselves to the input opt into borrowing here;
/// `#[serde(borrow)]` is what supplies the `'de: 'a` bound this needs.
///
/// ```
/// # use cashu::CheapStr;
/// #[derive(serde::Deserialize)]
/// struct Unit<'a> {
///     #[serde(borrow, deserialize_with = "cashu::cheap_str::deserialize_borrowed")]
///     name: CheapStr<'a>,
/// }
/// ```
pub fn deserialize_borrowed<'de: 'a, 'a, D>(deserializer: D) -> Result<CheapStr<'a>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_str(BorrowingVisitor(PhantomData))
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::collections::HashMap;

    use super::*;

    fn hash_of<T: Hash + ?Sized>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn variants_are_not_observable() {
        let owned = String::from("usd");
        let static_ = CheapStr::from_static("usd");
        let borrowed = CheapStr::from(owned.as_str());
        let arc = CheapStr::from(owned.clone());

        assert_eq!(static_, borrowed);
        assert_eq!(borrowed, arc);
        assert_eq!(static_, arc);

        assert_eq!(hash_of(&static_), hash_of(&borrowed));
        assert_eq!(hash_of(&borrowed), hash_of(&arc));
        assert_eq!(hash_of(&arc), hash_of("usd"));
    }

    #[test]
    fn ordering_is_lexicographic() {
        // `Owned` sorts after `Static` by discriminant, so a derived `Ord` would
        // get this backwards.
        let mut units = vec![
            CheapStr::from(String::from("zar")),
            CheapStr::from_static("aud"),
            CheapStr::from(String::from("eur")),
        ];
        units.sort();

        assert_eq!(
            units,
            vec![
                CheapStr::from_static("aud"),
                CheapStr::from_static("eur"),
                CheapStr::from_static("zar"),
            ]
        );
    }

    #[test]
    fn map_lookup_by_str() {
        let mut map = HashMap::new();
        map.insert(CheapStr::from(String::from("usd")), 1);

        assert_eq!(map.get("usd"), Some(&1));
        assert_eq!(map.get(&CheapStr::from_static("usd")), Some(&1));
    }

    #[test]
    fn serde_is_a_bare_string() {
        let value = CheapStr::from(String::from("usd"));
        assert_eq!(serde_json::to_string(&value).unwrap(), r#""usd""#);

        let decoded: CheapStr<'_> = serde_json::from_str(r#""usd""#).unwrap();
        assert_eq!(decoded, "usd");
    }

    #[test]
    fn deserialize_owns_even_when_the_input_can_lend() {
        let json = String::from(r#""usd""#);
        let decoded: CheapStr<'_> = serde_json::from_str(&json).unwrap();

        assert!(matches!(decoded.0, Inner::Owned(_)));
    }

    #[test]
    fn deserialize_borrowed_borrows_when_the_input_can_lend() {
        #[derive(Deserialize)]
        struct Wrapper<'a> {
            #[serde(borrow, deserialize_with = "deserialize_borrowed")]
            unit: CheapStr<'a>,
        }

        let json = String::from(r#"{"unit":"usd"}"#);
        let decoded: Wrapper<'_> = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.unit, "usd");
        assert!(matches!(decoded.unit.0, Inner::Borrowed(_)));
    }

    #[test]
    fn static_form_is_deserialize_owned() {
        #[derive(Deserialize)]
        struct Wrapper {
            unit: CheapStr<'static>,
        }

        fn assert_de_owned<T: serde::de::DeserializeOwned>() {}
        assert_de_owned::<CheapStr<'static>>();
        assert_de_owned::<Wrapper>();

        let json = String::from(r#"{"unit":"usd"}"#);
        let decoded: Wrapper = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.unit, "usd");

        let bare = String::from(r#""usd""#);
        let decoded: CheapStr<'static> = serde_json::from_str(&bare).unwrap();
        assert_eq!(decoded, "usd");
    }

    #[test]
    fn default_is_empty() {
        let value = CheapStr::default();
        assert!(value.is_empty());
        assert_eq!(value, "");
    }

    #[test]
    fn into_static_outlives_the_borrow() {
        let detached = {
            let scoped = String::from("usd");
            CheapStr::from(scoped.as_str()).into_static()
        };

        assert_eq!(detached, "usd");
    }

    #[test]
    fn into_static_keeps_literals_free() {
        let value = CheapStr::from_static("usd").into_static();
        assert!(matches!(value.0, Inner::Static(_)));
    }
}
