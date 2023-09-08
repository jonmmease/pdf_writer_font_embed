//! This example gives you a first introduction on how to use pdf-writer.

use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash};
use fontdb::Source;
use pdf_writer::types::{ActionType, AnnotationType, BorderType, CidFontType, FontFlags, SystemInfo, UnicodeCmap};
use pdf_writer::{Content, Filter, Finish, Name, PdfWriter, Rect, Ref, Str, TextStr};
use siphasher::sip128::{Hasher128, SipHasher13};
use ttf_parser::GlyphId;

const SYSTEM_INFO: SystemInfo = SystemInfo {
    registry: Str(b"Adobe"),
    ordering: Str(b"Identity"),
    supplement: 0,
};
const CMAP_NAME: Name = Name(b"Custom");


fn main() -> std::io::Result<()> {
    // load system fonts
    let mut font_db = fontdb::Database::new();
    font_db.load_system_fonts();

    // Query font database for a particular font
    let font_id = font_db.query(&fontdb::Query {
        families: &[fontdb::Family::Name("Helvetica")],
        weight: Default::default(),
        stretch: Default::default(),
        style: Default::default(),
    }).expect("Failed to find requested font");

    let face = font_db.face(font_id).unwrap();

    // Read source data
    let font_data = match &face.source {
        Source::Binary(b) => { Vec::from(b.as_ref().as_ref())}
        Source::File(f) => { fs::read(f).unwrap() }
        Source::SharedFile(f, _) => { fs::read(f).unwrap() }
    };

    // Parse as ttf font
    let ttf = ttf_parser::Face::parse(
        font_data.as_slice(), face.index
    ).expect("Failed to parse font data as ttf");

    // Conversion function from ttf values in em to PDFs font units
    let to_font_units = |v: f32| (v / ttf.units_per_em() as f32) * 1000.0;

    // Specify the string we want to display
    let message = "Hello World from Rust!";

    // Get Vec of the 16-bit glyph number for each unicode character
    let message_glyphs: Vec<_> = message.chars().map(|ch| ttf.glyph_index(ch).unwrap().0 ).collect();

    // Build mapping from glyph to unicode character string
    let mut glyph_set: BTreeMap<u16, String> = BTreeMap::new();
    for ch in message.chars() {
        let Some(glyph) = ttf.glyph_index(ch) else { continue };
        glyph_set.entry(glyph.0).or_insert_with(|| ch.to_string());
    }

    // Start writing PDF
    let mut writer = PdfWriter::new();

    // Define some indirect reference ids we'll use.
    let catalog_ref = Ref::new(1);
    let page_tree_ref = Ref::new(2);
    let page_ref = Ref::new(3);
    let type0_ref = Ref::new(4);
    let cid_ref = Ref::new(5);
    let descriptor_ref = Ref::new(6);
    let cmap_ref = Ref::new(7);
    let data_ref = Ref::new(8);
    let content_ref = Ref::new(9);

    let font_name = Name(b"F1");

    // Write the document catalog with a reference to the page tree.
    writer.catalog(catalog_ref).pages(page_tree_ref);

    // Write the page tree with a single child page.
    writer.pages(page_tree_ref).kids([page_ref]).count(1);

    // Write a page.
    let mut page = writer.page(page_ref);

    // Set the size to A4 (measured in points) using `media_box` and set the
    // text object we'll write later as the page's contents.
    page.media_box(Rect::new(0.0, 0.0, 595.0, 842.0));
    page.parent(page_tree_ref);
    page.contents(content_ref);

    // We also need to specify which resources the page needs, which in our case
    // is only a font that we name "F1" (the specific name doesn't matter).
    page.resources().fonts().pair(font_name, type0_ref);
    page.finish();

    // Specify the font we want to use. Because Helvetica is one of the 14 base
    // fonts shipped with every PDF reader, we don't have to embed any font
    // data.
    let postscript_name = face.post_script_name.clone();
    let subset_tag = subset_tag(&glyph_set);
    let base_font = format!("{subset_tag}+{postscript_name}");
    writer
        .type0_font(type0_ref)
        .base_font(Name(base_font.as_bytes()))
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(cid_ref)
        .to_unicode(cmap_ref);

    // Write the CID font referencing the font descriptor.
    let mut cid = writer.cid_font(cid_ref);
    cid.subtype( CidFontType::Type2);
    cid.base_font(Name(base_font.as_bytes()));
    cid.system_info(SYSTEM_INFO);
    cid.font_descriptor(descriptor_ref);
    cid.default_width(0.0);
    cid.cid_to_gid_map_predefined(Name(b"Identity"));

    // Compute widths
    let num_glyphs = ttf.number_of_glyphs();
    let mut widths = vec![0.0; num_glyphs as usize];
    for g in glyph_set.keys().copied() {
        let x= ttf.glyph_hor_advance(GlyphId(g)).unwrap_or(0);
        widths[g as usize] = to_font_units(x as f32);
    }

    // Write all non-zero glyph widths.
    let mut start = 0;
    let mut start_width = widths[0];
    let mut width_writer = cid.widths();
    for (i, w) in widths.iter().enumerate().skip(1) {
        if *w != start_width || i == widths.len() - 1 {
            if start_width != 0.0 {
                width_writer.same(start as u16, i as u16, start_width);
            }
            start = i as i32;
            start_width = *w;
        }
    }

    width_writer.finish();
    cid.finish();

    // Flags
    let mut flags = FontFlags::empty();
    flags.set(FontFlags::SERIF, postscript_name.contains("Serif"));
    flags.set(FontFlags::FIXED_PITCH, ttf.is_monospaced());
    flags.set(FontFlags::ITALIC, ttf.is_italic());
    flags.insert(FontFlags::SYMBOLIC);
    flags.insert(FontFlags::SMALL_CAP);

    // bounding box
    let global_bbox = ttf.global_bounding_box();
    let bbox = Rect::new(
        to_font_units(global_bbox.x_min.into()),
        to_font_units(global_bbox.y_min.into()),
        to_font_units(global_bbox.x_max.into()),
        to_font_units(global_bbox.y_max.into()),
    );

    let italic_angle = ttf.italic_angle().unwrap_or(0.0);
    let ascender = to_font_units(ttf.typographic_ascender().unwrap_or(ttf.ascender()).into());
    let descender = to_font_units(ttf.typographic_descender().unwrap_or(ttf.descender()).into());
    let cap_height = to_font_units(ttf.capital_height().unwrap_or(ttf.ascender()).into());
    let stem_v = 10.0 + 0.244 * (f32::from(ttf.weight().to_number()) - 50.0);

    // Write the font descriptor (contains metrics about the font).
    let mut font_descriptor = writer.font_descriptor(descriptor_ref);
    font_descriptor
        .name(Name(base_font.as_bytes()))
        .flags(flags)
        .bbox(bbox)
        .italic_angle(italic_angle)
        .ascent(ascender)
        .descent(descender)
        .cap_height(cap_height)
        .stem_v(stem_v);

    font_descriptor.font_file2(data_ref);
    font_descriptor.finish();

    // Write the /ToUnicode character map, which maps glyph ids back to
    // unicode codepoints to enable copying out of the PDF.
    let cmap = create_cmap(&glyph_set);
    writer.cmap(cmap_ref, &cmap.finish());

    let glyphs: Vec<_> = glyph_set.keys().copied().collect();
    let profile = subsetter::Profile::pdf(&glyphs);
    let subsetted = subsetter::subset(&font_data, face.index, profile);
    let mut subset_font_data = deflate(subsetted.as_deref().unwrap_or(&font_data));

    // println!("subset_font_data: {:?}", &subset_font_data[..20]);
    let mut stream = writer.stream(data_ref, &subset_font_data);
    stream.filter(Filter::FlateDecode);
    stream.finish();

    // Encode u16 glyphs as pairs of u8 bytes
    let mut encoded = vec![];
    for g in message_glyphs {
        encoded.push((g >> 8) as u8);
        encoded.push((g & 0xff) as u8);
    }

    let mut content = Content::new();
    content.begin_text();
    content.set_font(font_name, 14.0);
    content.next_line(108.0, 734.0);
    content.show(Str(encoded.as_slice()));
    content.end_text();
    writer.stream(content_ref, &content.finish());

    // Finish writing (this automatically creates the cross-reference table and
    // file trailer) and retrieve the resulting byte buffer.
    let buf: Vec<u8> = writer.finish();

    // Write the thing to a file.
    fs::write("target/hello_embed.pdf", buf)
}

fn deflate(data: &[u8]) -> Vec<u8> {
    const COMPRESSION_LEVEL: u8 = 6;
    miniz_oxide::deflate::compress_to_vec_zlib(data, COMPRESSION_LEVEL)
}

/// Create a /ToUnicode CMap.
fn create_cmap(
    glyph_set: &BTreeMap<u16, String>,
) -> UnicodeCmap {

    // Produce a reverse mapping from glyphs to unicode strings.
    let mut cmap = UnicodeCmap::new(CMAP_NAME, SYSTEM_INFO);
    for (&g, text) in glyph_set.iter() {
        if !text.is_empty() {
            cmap.pair_with_multiple(g, text.chars());
        }
    }

    cmap
}

/// Produce a unique 6 letter tag for a glyph set.
fn subset_tag(glyphs: &BTreeMap<u16, String>) -> String {
    const LEN: usize = 6;
    const BASE: u128 = 26;
    let mut hash = hash128(glyphs);
    let mut letter = [b'A'; LEN];
    for l in letter.iter_mut() {
        *l = b'A' + (hash % BASE) as u8;
        hash /= BASE;
    }
    std::str::from_utf8(&letter).unwrap().to_string()
}

/// Calculate a 128-bit siphash of a value.
pub fn hash128<T: Hash + ?Sized>(value: &T) -> u128 {
    let mut state = SipHasher13::new();
    value.hash(&mut state);
    state.finish128().as_u128()
}
