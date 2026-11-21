use anyhow::anyhow;
use collections::HashSet;
use core_foundation::{
    array::{
        CFArray, CFArrayAppendValue, CFArrayCreateMutable, CFArrayGetCount, CFArrayGetValueAtIndex,
        CFArrayRef, CFMutableArrayRef, kCFTypeArrayCallBacks,
    },
    base::{CFRelease, TCFType, kCFAllocatorDefault},
    data::{CFData, CFDataRef},
    dictionary::{
        CFDictionaryCreate, CFDictionaryGetCount, CFDictionaryRef, kCFTypeDictionaryKeyCallBacks,
        kCFTypeDictionaryValueCallBacks,
    },
    number::CFNumber,
    string::CFString,
};
use core_foundation_sys::locale::CFLocaleCopyPreferredLanguages;
use core_graphics::{data_provider::CGDataProvider, font::CGFont};
use core_text::font_descriptor::{
    TraitAccessors, kCTFontFamilyNameAttribute, kCTFontItalicTrait, kCTFontSlantTrait,
    kCTFontTraitsAttribute, kCTFontWeightTrait,
};
use core_text::{
    font::{CTFont, CTFontRef, cascade_list_for_languages},
    font_descriptor::{
        CTFontDescriptor, CTFontDescriptorCreateWithAttributes, kCTFontCascadeListAttribute,
        kCTFontFeatureSettingsAttribute,
    },
};
use font_kit::font::Font as FontKitFont;
use gpui::{FontFallbacks, FontFeatures};
use objc2_core_text::{
    kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue, kCTFontVariationAttribute,
};
use std::sync::Arc;

const TAG_GDEF: u32 = u32::from_be_bytes(*b"GDEF");
const TAG_CFF: u32 = u32::from_be_bytes(*b"CFF ");
const TAG_CFF2: u32 = u32::from_be_bytes(*b"CFF2");
const TAG_HEAD: u32 = u32::from_be_bytes(*b"head");
const SFNT_CHECKSUM_ADJUSTMENT_MAGIC: u32 = 0xB1B0_AFBA;

const GDEF_CLASS_BASE: u16 = 1;
const GDEF_CLASS_MARK: u16 = 3;

unsafe extern "C" {
    fn CTFontCopyTable(font: CTFontRef, table: u32, options: u32) -> CFDataRef;
    fn CTFontCopyAvailableTables(font: CTFontRef, options: u32) -> CFArrayRef;
    fn CTFontCopyVariation(font: CTFontRef) -> CFDictionaryRef;
}

/// Reclassifies standalone spacing characters (such as ASCII backtick U+0060) that are
/// misclassified as OpenType GDEF Class 3 (`Mark`) to Class 1 (`Base`). CoreText zeroes the
/// advance width of all GDEF Class 3 glyphs during GPOS shaping, which otherwise collapses
/// spacing glyphs with positive `hmtx` advance into overlapping zero-width marks.
pub fn sanitize_gdef_spacing_marks(font: &mut FontKitFont) -> anyhow::Result<()> {
    let ct_font = font.native_font();
    let Some(gdef_data) = copy_font_table(&ct_font, TAG_GDEF) else {
        return Ok(());
    };
    let Some(mark_glyphs) = collect_gdef_mark_glyphs(&gdef_data) else {
        return Ok(());
    };

    let misclassified = collect_misclassified_spacing_glyphs(font, &mark_glyphs);
    if misclassified.is_empty() {
        return Ok(());
    }

    let Some(patched_gdef) = patch_gdef_spacing_marks(&gdef_data, &misclassified) else {
        return Ok(());
    };
    let Some(sfnt_bytes) = rebuild_sfnt_with_patched_gdef(&ct_font, patched_gdef) else {
        return Ok(());
    };

    let data_provider = CGDataProvider::from_buffer(Arc::new(sfnt_bytes));
    let cg_font = CGFont::from_data_provider(data_provider)
        .map_err(|()| anyhow!("Could not construct CGFont from sanitized sfnt data"))?;
    let mut sanitized = font_kit::loaders::core_text::Font::from_core_graphics_font(cg_font);
    preserve_variation_coordinates(&ct_font, &mut sanitized);
    *font = sanitized;
    Ok(())
}

fn copy_font_table(ct_font: &CTFont, tag: u32) -> Option<Vec<u8>> {
    let data_ref = unsafe { CTFontCopyTable(ct_font.as_concrete_TypeRef(), tag, 0) };
    if data_ref.is_null() {
        return None;
    }
    let cf_data = unsafe { CFData::wrap_under_create_rule(data_ref) };
    Some(cf_data.bytes().to_vec())
}

fn read_u16_be(bytes: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let slice = bytes.get(offset..end)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

fn write_u16_be(bytes: &mut [u8], offset: usize, value: u16) -> Option<()> {
    let end = offset.checked_add(2)?;
    let slice = bytes.get_mut(offset..end)?;
    slice.copy_from_slice(&value.to_be_bytes());
    Some(())
}

fn collect_gdef_mark_glyphs(gdef: &[u8]) -> Option<HashSet<u16>> {
    if gdef.len() < 12 || read_u16_be(gdef, 0) != Some(1) {
        return None;
    }
    let gc_off = usize::from(read_u16_be(gdef, 4)?);
    if gc_off == 0 || gc_off.checked_add(4)? > gdef.len() {
        return None;
    }

    let mut marks = HashSet::default();
    match read_u16_be(gdef, gc_off)? {
        1 => {
            let start_glyph = read_u16_be(gdef, gc_off + 2)?;
            let glyph_count = usize::from(read_u16_be(gdef, gc_off + 4)?);
            let table_end = gc_off
                .checked_add(6)?
                .checked_add(glyph_count.checked_mul(2)?)?;
            if table_end > gdef.len() {
                return None;
            }
            for index in 0..glyph_count {
                if read_u16_be(gdef, gc_off + 6 + index * 2) == Some(GDEF_CLASS_MARK)
                    && let Ok(offset_u16) = u16::try_from(index)
                    && let Some(gid) = start_glyph.checked_add(offset_u16)
                {
                    marks.insert(gid);
                }
            }
        }
        2 => {
            let range_count = usize::from(read_u16_be(gdef, gc_off + 2)?);
            let table_end = gc_off
                .checked_add(4)?
                .checked_add(range_count.checked_mul(6)?)?;
            if table_end > gdef.len() {
                return None;
            }
            for record in 0..range_count {
                let record_off = gc_off + 4 + record * 6;
                let start_gid = read_u16_be(gdef, record_off)?;
                let end_gid = read_u16_be(gdef, record_off + 2)?;
                let class = read_u16_be(gdef, record_off + 4)?;
                if class == GDEF_CLASS_MARK && start_gid <= end_gid {
                    for gid in start_gid..=end_gid {
                        marks.insert(gid);
                    }
                }
            }
        }
        _ => return None,
    }

    (!marks.is_empty()).then_some(marks)
}

fn collect_misclassified_spacing_glyphs(
    font: &FontKitFont,
    mark_glyphs: &HashSet<u16>,
) -> HashSet<u16> {
    let mut misclassified = HashSet::default();
    let ranges = [
        0x0021u32..=0x007E,
        0x00A1u32..=0x00AC,
        0x00AEu32..=0x00FF,
        0x02B0u32..=0x02FF,
    ];
    for range in ranges {
        for codepoint in range {
            let Some(ch) = char::from_u32(codepoint) else {
                continue;
            };
            let Some(glyph_id) = font.glyph_for_char(ch) else {
                continue;
            };
            let Ok(gid_u16) = u16::try_from(glyph_id) else {
                continue;
            };
            if gid_u16 != 0
                && mark_glyphs.contains(&gid_u16)
                && font.advance(glyph_id).is_ok_and(|adv| adv.x() > 0.0)
            {
                misclassified.insert(gid_u16);
            }
        }
    }
    misclassified
}

fn push_class_range(ranges: &mut Vec<(u16, u16, u16)>, start: u16, end: u16, class: u16) {
    if let Some(last) = ranges.last_mut()
        && last.2 == class
        && last.1.checked_add(1) == Some(start)
    {
        last.1 = end;
        return;
    }
    ranges.push((start, end, class));
}

fn patch_gdef_spacing_marks(gdef: &[u8], misclassified: &HashSet<u16>) -> Option<Vec<u8>> {
    let gc_off = usize::from(read_u16_be(gdef, 4)?);
    let mut patched = gdef.to_vec();

    match read_u16_be(&patched, gc_off)? {
        1 => {
            let start_glyph = read_u16_be(&patched, gc_off + 2)?;
            let glyph_count = usize::from(read_u16_be(&patched, gc_off + 4)?);
            let mut changed = false;
            for &gid in misclassified {
                if let Some(delta) = gid.checked_sub(start_glyph) {
                    let index = usize::from(delta);
                    if index < glyph_count
                        && read_u16_be(&patched, gc_off + 6 + index * 2) == Some(GDEF_CLASS_MARK)
                    {
                        write_u16_be(&mut patched, gc_off + 6 + index * 2, GDEF_CLASS_BASE)?;
                        changed = true;
                    }
                }
            }
            changed.then_some(patched)
        }
        2 => {
            let range_count = usize::from(read_u16_be(&patched, gc_off + 2)?);
            let mut merged_ranges: Vec<(u16, u16, u16)> = Vec::with_capacity(range_count + 4);
            let mut changed = false;

            for record in 0..range_count {
                let record_off = gc_off + 4 + record * 6;
                let start_gid = read_u16_be(&patched, record_off)?;
                let end_gid = read_u16_be(&patched, record_off + 2)?;
                let class = read_u16_be(&patched, record_off + 4)?;
                if class == GDEF_CLASS_MARK
                    && start_gid <= end_gid
                    && (start_gid..=end_gid).any(|gid| misclassified.contains(&gid))
                {
                    changed = true;
                    for gid in start_gid..=end_gid {
                        let next_class = if misclassified.contains(&gid) {
                            GDEF_CLASS_BASE
                        } else {
                            GDEF_CLASS_MARK
                        };
                        push_class_range(&mut merged_ranges, gid, gid, next_class);
                    }
                } else {
                    push_class_range(&mut merged_ranges, start_gid, end_gid, class);
                }
            }

            if !changed {
                return None;
            }

            let new_count = u16::try_from(merged_ranges.len()).ok()?;
            if merged_ranges.len() <= range_count {
                write_u16_be(&mut patched, gc_off + 2, new_count)?;
                for (index, &(start_gid, end_gid, class)) in merged_ranges.iter().enumerate() {
                    let record_off = gc_off + 4 + index * 6;
                    write_u16_be(&mut patched, record_off, start_gid)?;
                    write_u16_be(&mut patched, record_off + 2, end_gid)?;
                    write_u16_be(&mut patched, record_off + 4, class)?;
                }
                let used_end = gc_off + 4 + merged_ranges.len() * 6;
                let orig_end = gc_off + 4 + range_count * 6;
                if let Some(slack) = patched.get_mut(used_end..orig_end) {
                    slack.fill(0);
                }
            } else {
                if !patched.len().is_multiple_of(2) {
                    patched.push(0);
                }
                let new_gc_off = u16::try_from(patched.len()).ok()?;
                write_u16_be(&mut patched, 4, new_gc_off)?;
                patched.extend_from_slice(&2u16.to_be_bytes());
                patched.extend_from_slice(&new_count.to_be_bytes());
                for (start_gid, end_gid, class) in merged_ranges {
                    patched.extend_from_slice(&start_gid.to_be_bytes());
                    patched.extend_from_slice(&end_gid.to_be_bytes());
                    patched.extend_from_slice(&class.to_be_bytes());
                }
            }
            Some(patched)
        }
        _ => None,
    }
}

fn sfnt_checksum(bytes: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for chunk in bytes.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

fn rebuild_sfnt_with_patched_gdef(ct_font: &CTFont, patched_gdef: Vec<u8>) -> Option<Vec<u8>> {
    let tables_ref = unsafe { CTFontCopyAvailableTables(ct_font.as_concrete_TypeRef(), 0) };
    if tables_ref.is_null() {
        return None;
    }
    let table_count = unsafe { CFArrayGetCount(tables_ref) };
    let mut tags = Vec::with_capacity(usize::try_from(table_count).ok()?);
    for index in 0..table_count {
        let raw_tag = unsafe { CFArrayGetValueAtIndex(tables_ref, index) };
        let tag = u32::try_from(raw_tag as usize).ok()?;
        tags.push(tag);
    }
    unsafe {
        CFRelease(tables_ref.cast());
    }

    let mut tables: Vec<(u32, Vec<u8>)> = Vec::with_capacity(tags.len());
    let mut has_cff = false;
    for tag in tags {
        if tag == TAG_CFF || tag == TAG_CFF2 {
            has_cff = true;
        }
        if tag == TAG_GDEF {
            tables.push((tag, patched_gdef.clone()));
            continue;
        }
        let mut table_bytes = copy_font_table(ct_font, tag)?;
        if tag == TAG_HEAD
            && let Some(adj) = table_bytes.get_mut(8..12)
        {
            adj.fill(0);
        }
        tables.push((tag, table_bytes));
    }

    tables.sort_by_key(|(tag, _)| *tag);
    let num_tables = u16::try_from(tables.len()).ok()?;
    let mut search_range: u16 = 16;
    let mut entry_selector: u16 = 0;
    while let Some(doubled) = search_range.checked_mul(2)
        && doubled <= num_tables.saturating_mul(16)
    {
        search_range = doubled;
        entry_selector += 1;
    }
    let range_shift = num_tables.saturating_mul(16).saturating_sub(search_range);

    let sfnt_version: u32 = if has_cff { 0x4F54_544F } else { 0x0001_0000 };
    let header_len = 12 + usize::from(num_tables) * 16;
    let mut sfnt =
        Vec::with_capacity(header_len + tables.iter().map(|(_, b)| b.len() + 3).sum::<usize>());
    sfnt.extend_from_slice(&sfnt_version.to_be_bytes());
    sfnt.extend_from_slice(&num_tables.to_be_bytes());
    sfnt.extend_from_slice(&search_range.to_be_bytes());
    sfnt.extend_from_slice(&entry_selector.to_be_bytes());
    sfnt.extend_from_slice(&range_shift.to_be_bytes());

    let mut current_offset = header_len;
    let mut head_offset_and_len: Option<(usize, usize)> = None;
    for (tag, data) in &tables {
        let checksum = sfnt_checksum(data);
        let offset_u32 = u32::try_from(current_offset).ok()?;
        let length_u32 = u32::try_from(data.len()).ok()?;
        if *tag == TAG_HEAD {
            head_offset_and_len = Some((current_offset, data.len()));
        }
        sfnt.extend_from_slice(&tag.to_be_bytes());
        sfnt.extend_from_slice(&checksum.to_be_bytes());
        sfnt.extend_from_slice(&offset_u32.to_be_bytes());
        sfnt.extend_from_slice(&length_u32.to_be_bytes());
        let padded_len = (data.len() + 3) & !3;
        current_offset = current_offset.checked_add(padded_len)?;
    }

    for (_, data) in &tables {
        sfnt.extend_from_slice(data);
        let pad = (4 - (data.len() % 4)) % 4;
        sfnt.resize(sfnt.len() + pad, 0);
    }

    if let Some((head_offset, head_len)) = head_offset_and_len
        && head_len >= 12
    {
        let adjustment = SFNT_CHECKSUM_ADJUSTMENT_MAGIC.wrapping_sub(sfnt_checksum(&sfnt));
        if let Some(adj) = sfnt.get_mut(head_offset + 8..head_offset + 12) {
            adj.copy_from_slice(&adjustment.to_be_bytes());
        }
    }

    Some(sfnt)
}

fn preserve_variation_coordinates(original: &CTFont, sanitized: &mut FontKitFont) {
    let variation_ref = unsafe { CTFontCopyVariation(original.as_concrete_TypeRef()) };
    if variation_ref.is_null() {
        return;
    }
    let count = unsafe { CFDictionaryGetCount(variation_ref) };
    if count <= 0 {
        unsafe {
            CFRelease(variation_ref.cast());
        }
        return;
    }

    unsafe {
        let keys = [kCTFontVariationAttribute as *const _];
        let values = [variation_ref as *const _];
        let attrs = CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr() as _,
            values.as_ptr() as _,
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        CFRelease(variation_ref.cast());
        let descriptor = CTFontDescriptorCreateWithAttributes(attrs);
        CFRelease(attrs.cast());
        let descriptor = CTFontDescriptor::wrap_under_create_rule(descriptor);
        let native_font = sanitized.native_font();
        let generated_font = &*native_font
            .as_concrete_TypeRef()
            .cast::<objc2_core_text::CTFont>();
        let generated_desc = &*descriptor
            .as_concrete_TypeRef()
            .cast::<objc2_core_text::CTFontDescriptor>();
        let new_font =
            generated_font.copy_with_attributes(0.0, std::ptr::null(), Some(generated_desc));
        let new_font = CTFont::wrap_under_create_rule(
            objc2_core_foundation::CFRetained::into_raw(new_font)
                .as_ptr()
                .cast(),
        );
        *sanitized = font_kit::font::Font::from_native_font(&new_font);
    }
}

pub fn apply_features_and_fallbacks(
    font: &mut FontKitFont,
    features: &FontFeatures,
    fallbacks: Option<&FontFallbacks>,
) -> anyhow::Result<()> {
    unsafe {
        let mut keys = vec![kCTFontFeatureSettingsAttribute];
        let mut values = vec![generate_feature_array(features)];
        if let Some(fallbacks) = fallbacks
            && !fallbacks.fallback_list().is_empty()
        {
            keys.push(kCTFontCascadeListAttribute);
            values.push(generate_fallback_array(fallbacks, font));
        }
        let attrs = CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr() as _,
            values.as_ptr() as _,
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );

        for value in &values {
            CFRelease(*value as _);
        }

        let new_descriptor = CTFontDescriptorCreateWithAttributes(attrs);
        CFRelease(attrs as _);
        let new_descriptor = CTFontDescriptor::wrap_under_create_rule(new_descriptor);
        // font-kit uses the older core-text wrappers. Borrow the same CF objects
        // for the generated API, then transfer its create-rule ownership back.
        let native_font = font.native_font();
        let generated_font = &*native_font
            .as_concrete_TypeRef()
            .cast::<objc2_core_text::CTFont>();
        let descriptor = &*new_descriptor
            .as_concrete_TypeRef()
            .cast::<objc2_core_text::CTFontDescriptor>();
        let new_font = generated_font.copy_with_attributes(0.0, std::ptr::null(), Some(descriptor));
        let new_font = CTFont::wrap_under_create_rule(
            objc2_core_foundation::CFRetained::into_raw(new_font)
                .as_ptr()
                .cast(),
        );
        *font = font_kit::font::Font::from_native_font(&new_font);

        Ok(())
    }
}

fn generate_feature_array(features: &FontFeatures) -> CFMutableArrayRef {
    unsafe {
        let feature_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for (tag, value) in features.tag_value_list() {
            let keys = [
                kCTFontOpenTypeFeatureTag as *const _,
                kCTFontOpenTypeFeatureValue as *const _,
            ];
            let tag = CFString::new(tag);
            let value = CFNumber::from(*value as i32);
            let values = [tag.as_CFTypeRef(), value.as_CFTypeRef()];
            let dict = CFDictionaryCreate(
                kCFAllocatorDefault,
                &keys as *const _ as _,
                &values as *const _ as _,
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFArrayAppendValue(feature_array, dict as _);
            CFRelease(dict as _);
        }
        feature_array
    }
}

fn generate_fallback_array(fallbacks: &FontFallbacks, font: &mut FontKitFont) -> CFMutableArrayRef {
    unsafe {
        let symbolic_traits = font.native_font().symbolic_traits();
        let all_traits = font.native_font().all_traits();

        let fallback_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for user_fallback in fallbacks.fallback_list() {
            let name = CFString::from(user_fallback.as_str());

            let traits_keys = [kCTFontWeightTrait, kCTFontSlantTrait];
            let weight_value = CFNumber::from(all_traits.normalized_weight());
            let slant_value = CFNumber::from(if (symbolic_traits & kCTFontItalicTrait) != 0 {
                1.0
            } else {
                0.0
            });
            let traits_values = [weight_value.as_CFTypeRef(), slant_value.as_CFTypeRef()];
            let traits = CFDictionaryCreate(
                kCFAllocatorDefault,
                &traits_keys as *const _ as _,
                &traits_values as *const _ as _,
                traits_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            drop(weight_value);
            drop(slant_value);

            let attr_keys = [kCTFontFamilyNameAttribute, kCTFontTraitsAttribute];
            let attr_values = [name.as_CFTypeRef(), traits as _];
            let attrs = CFDictionaryCreate(
                kCFAllocatorDefault,
                &attr_keys as *const _ as _,
                &attr_values as *const _ as _,
                attr_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(traits as _);

            let fallback_desc = CTFontDescriptorCreateWithAttributes(attrs);
            CFRelease(attrs as _);

            CFArrayAppendValue(fallback_array, fallback_desc as _);
            CFRelease(fallback_desc as _);
        }

        append_system_fallbacks(fallback_array, &font.native_font());
        fallback_array
    }
}

fn append_system_fallbacks(fallback_array: CFMutableArrayRef, font: &CTFont) {
    unsafe {
        let preferred_languages: CFArray<CFString> =
            CFArray::wrap_under_create_rule(CFLocaleCopyPreferredLanguages());

        let default_fallbacks = cascade_list_for_languages(font, &preferred_languages);

        for desc in default_fallbacks
            .iter()
            .filter(|desc| desc.font_path().is_some())
        {
            CFArrayAppendValue(fallback_array, desc.as_concrete_TypeRef() as _);
        }
    }
}
