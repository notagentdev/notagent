const IMAGE_TYPE_SNIFF_BYTES: usize = 4100;
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// The inline image type of `buffer`, or `None` when it is not a supported image.
pub fn detect_supported_image_mime_type(buffer: &[u8]) -> Option<&'static str> {
    if starts_with(buffer, &[0xff, 0xd8, 0xff]) {
        // 0xf7 marks a JPEG-LS stream, which providers do not accept.
        return (buffer.get(3) != Some(&0xf7)).then_some("image/jpeg");
    }
    if starts_with(buffer, &PNG_SIGNATURE) {
        return (is_png(buffer) && !is_animated_png(buffer)).then_some("image/png");
    }
    if starts_with_ascii(buffer, 0, "GIF") {
        return Some("image/gif");
    }
    if starts_with_ascii(buffer, 0, "RIFF") && starts_with_ascii(buffer, 8, "WEBP") {
        return Some("image/webp");
    }
    if starts_with_ascii(buffer, 0, "BM") && is_bmp(buffer) {
        return Some("image/bmp");
    }
    None
}

pub async fn detect_supported_image_mime_type_from_file(file_path: &str) -> Option<&'static str> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(file_path).await.ok()?;
    let mut buffer = vec![0u8; IMAGE_TYPE_SNIFF_BYTES];
    let mut filled = 0usize;
    // the buffer for regular files.
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]).await {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    detect_supported_image_mime_type(&buffer[..filled])
}

fn is_png(buffer: &[u8]) -> bool {
    buffer.len() >= 16
        && read_u32_be(buffer, PNG_SIGNATURE.len()) == 13
        && starts_with_ascii(buffer, 12, "IHDR")
}

fn is_animated_png(buffer: &[u8]) -> bool {
    let mut offset = PNG_SIGNATURE.len();
    while offset + 8 <= buffer.len() {
        let chunk_length = read_u32_be(buffer, offset) as usize;
        let chunk_type_offset = offset + 4;
        if starts_with_ascii(buffer, chunk_type_offset, "acTL") {
            return true;
        }
        if starts_with_ascii(buffer, chunk_type_offset, "IDAT") {
            return false;
        }
        let Some(next_offset) = offset
            .checked_add(8)
            .and_then(|next| next.checked_add(chunk_length))
            .and_then(|next| next.checked_add(4))
        else {
            return false;
        };
        if next_offset <= offset || next_offset > buffer.len() {
            return false;
        }
        offset = next_offset;
    }
    false
}

fn is_bmp(buffer: &[u8]) -> bool {
    if buffer.len() < 26 {
        return false;
    }
    let declared_file_size = read_u32_le(buffer, 2);
    let pixel_data_offset = read_u32_le(buffer, 10);
    let dib_header_size = read_u32_le(buffer, 14);
    if declared_file_size != 0 && declared_file_size < 26 {
        return false;
    }
    if pixel_data_offset < 14 + dib_header_size {
        return false;
    }
    if declared_file_size != 0 && pixel_data_offset >= declared_file_size {
        return false;
    }

    let (color_planes, bits_per_pixel) = if dib_header_size == 12 {
        (read_u16_le(buffer, 22), read_u16_le(buffer, 24))
    } else if (40..=124).contains(&dib_header_size) {
        if buffer.len() < 30 {
            return false;
        }
        (read_u16_le(buffer, 26), read_u16_le(buffer, 28))
    } else {
        return false;
    };

    color_planes == 1 && matches!(bits_per_pixel, 1 | 4 | 8 | 16 | 24 | 32)
}

fn read_u16_le(buffer: &[u8], offset: usize) -> u32 {
    u32::from(byte(buffer, offset)) + (u32::from(byte(buffer, offset + 1)) << 8)
}

fn read_u32_be(buffer: &[u8], offset: usize) -> u32 {
    u32::from(byte(buffer, offset)).wrapping_mul(0x0100_0000)
        + (u32::from(byte(buffer, offset + 1)) << 16)
        + (u32::from(byte(buffer, offset + 2)) << 8)
        + u32::from(byte(buffer, offset + 3))
}

fn read_u32_le(buffer: &[u8], offset: usize) -> u32 {
    u32::from(byte(buffer, offset))
        + (u32::from(byte(buffer, offset + 1)) << 8)
        + (u32::from(byte(buffer, offset + 2)) << 16)
        + u32::from(byte(buffer, offset + 3)).wrapping_mul(0x0100_0000)
}

fn byte(buffer: &[u8], offset: usize) -> u8 {
    buffer.get(offset).copied().unwrap_or(0)
}

fn starts_with(buffer: &[u8], bytes: &[u8]) -> bool {
    buffer.len() >= bytes.len() && buffer[..bytes.len()] == *bytes
}

fn starts_with_ascii(buffer: &[u8], offset: usize, text: &str) -> bool {
    buffer.len() >= offset + text.len() && &buffer[offset..offset + text.len()] == text.as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_header(extra_chunks: &[(&str, usize)]) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0u8; 13]);
        bytes.extend_from_slice(&[0u8; 4]);
        for (chunk_type, length) in extra_chunks {
            bytes.extend_from_slice(&(*length as u32).to_be_bytes());
            bytes.extend_from_slice(chunk_type.as_bytes());
            bytes.extend(std::iter::repeat_n(0u8, *length));
            bytes.extend_from_slice(&[0u8; 4]);
        }
        bytes
    }

    #[test]
    fn detects_jpeg_but_rejects_jpeg_ls() {
        assert_eq!(
            detect_supported_image_mime_type(&[0xff, 0xd8, 0xff, 0xe0]),
            Some("image/jpeg")
        );
        assert_eq!(
            detect_supported_image_mime_type(&[0xff, 0xd8, 0xff, 0xf7]),
            None
        );
    }

    #[test]
    fn detects_png_and_rejects_animated_png() {
        assert_eq!(
            detect_supported_image_mime_type(&png_header(&[])),
            Some("image/png")
        );
        assert_eq!(
            detect_supported_image_mime_type(&png_header(&[("acTL", 8)])),
            None,
            "APNG is rejected"
        );
        // An IDAT chunk before acTL means the file is a still image.
        assert_eq!(
            detect_supported_image_mime_type(&png_header(&[("IDAT", 4), ("acTL", 8)])),
            Some("image/png")
        );
        // A PNG signature without a valid IHDR is not accepted.
        assert_eq!(detect_supported_image_mime_type(&PNG_SIGNATURE), None);
    }

    #[test]
    fn detects_gif_and_webp() {
        assert_eq!(
            detect_supported_image_mime_type(b"GIF89a"),
            Some("image/gif")
        );
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0u8; 4]);
        webp.extend_from_slice(b"WEBPVP8 ");
        assert_eq!(detect_supported_image_mime_type(&webp), Some("image/webp"));
        assert_eq!(detect_supported_image_mime_type(b"RIFFxxxxAVI "), None);
    }

    #[test]
    fn detects_bmp_with_a_plausible_header() {
        let mut bmp = vec![0u8; 30];
        bmp[0] = b'B';
        bmp[1] = b'M';
        bmp[2..6].copy_from_slice(&100u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        assert_eq!(detect_supported_image_mime_type(&bmp), Some("image/bmp"));

        // 3 colour planes are not valid.
        bmp[26..28].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(detect_supported_image_mime_type(&bmp), None);
    }

    #[test]
    fn rejects_other_content() {
        assert_eq!(detect_supported_image_mime_type(b"not an image"), None);
        assert_eq!(detect_supported_image_mime_type(&[]), None);
    }

    #[tokio::test]
    async fn sniffs_a_file_from_disk() {
        let directory = tempfile::Builder::new()
            .prefix("notagent-mime-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().join("image.png");
        std::fs::write(&path, png_header(&[])).expect("write");
        assert_eq!(
            detect_supported_image_mime_type_from_file(&path.to_string_lossy()).await,
            Some("image/png")
        );
        let text = directory.path().join("file.txt");
        std::fs::write(&text, "hello").expect("write");
        assert_eq!(
            detect_supported_image_mime_type_from_file(&text.to_string_lossy()).await,
            None
        );
        assert_eq!(
            detect_supported_image_mime_type_from_file("/missing/file.png").await,
            None
        );
    }
}
