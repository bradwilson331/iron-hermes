//! Static asset registration for the IronHermes font set.
//!
//! Without these declarations, `dx serve` falls through to the SPA index.html
//! for `/assets/fonts/*.woff2` requests because the macro is what registers
//! files with the dev server's router. `with_hash_suffix(false)` preserves
//! the original filenames so the verbatim `@font-face url("fonts/...")`
//! references in `assets/design-tokens.css` continue to resolve.

use dioxus::prelude::*;

const _IOSKELEY_MONO_FONTS: &[Asset] = &[
    asset!("/assets/fonts/IoskeleyMono-Black.woff2",           AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-Bold.woff2",            AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-BoldItalic.woff2",      AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-Condensed.woff2",       AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-CondensedBold.woff2",   AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-CondensedMedium.woff2", AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-ExtraBold.woff2",       AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-ExtraLight.woff2",      AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-Italic.woff2",          AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-Light.woff2",           AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-Medium.woff2",          AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-Regular.woff2",         AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-SemiBold.woff2",        AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-SemiCondensed.woff2",   AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-SemiLight.woff2",       AssetOptions::builder().with_hash_suffix(false)),
    asset!("/assets/fonts/IoskeleyMono-Thin.woff2",            AssetOptions::builder().with_hash_suffix(false)),
];
