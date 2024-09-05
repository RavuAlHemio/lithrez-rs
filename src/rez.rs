use std::borrow::Cow;
use std::fmt;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

use from_to_repr::from_to_other;
use smallvec::SmallVec;

use crate::io_ext::ReadExt;


const HEAD_TAIL_XOR: u8 = 0x11;
const ENCODE_VALUE_XOR: u32 = 0x016B4423;


#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidControlByte { index: usize, expected_one_of: [u8; 2], obtained: u8 },
    InvalidVersion { expected: u32, obtained: u32 },
    InvalidDetectHead { head: u8, detect_head: u8 },
    InvalidDetectTail { tail: u8, detect_tail: u8 },
    InvalidEncodeUtf8 { detection_value: bool, bytes: Vec<u8> },
    InvalidEncodeInteger { detection_value: bool, string: String },
    EncodeValueMismatch { encode_value: u32, detect_encode_value: u32 },
    UnknownEntryType { type_code: u32 },
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::InvalidControlByte { index, expected_one_of, obtained }
                => write!(f, "invalid control byte {} (expected one of {:?}, obtained {})", index, expected_one_of, obtained),
            Self::InvalidVersion { expected, obtained }
                => write!(f, "invalid version {} (expected {})", obtained, expected),
            Self::InvalidDetectHead { head, detect_head }
                => write!(f, "invalid DetectHead (head: 0x{:02X}, detect_head: 0x{:02X}, xor: 0x{:02X})", head, detect_head, head ^ HEAD_TAIL_XOR),
            Self::InvalidDetectTail { tail, detect_tail }
                => write!(f, "invalid DetectTail (head: 0x{:02X}, detect_head: 0x{:02X}, xor: 0x{:02X})", tail, detect_tail, tail ^ HEAD_TAIL_XOR),
            Self::InvalidEncodeUtf8 { detection_value, bytes }
                => write!(f, "invalid \"encode\"{} value UTF-8 bytes: {:?}", if *detection_value { " detection" } else { "" }, bytes),
            Self::InvalidEncodeInteger { detection_value, string }
                => write!(f, "invalid \"encode\"{} value integer: {:?}", if *detection_value { " detection" } else { "" }, string),
            Self::EncodeValueMismatch { encode_value, detect_encode_value }
                => write!(f, "encode value mismatch (encode value 0x{:08X}, xor'ed encode value 0x{:08X}, detect encode value 0x{:08X}", encode_value, encode_value ^ ENCODE_VALUE_XOR, detect_encode_value),
            Self::UnknownEntryType { type_code }
                => write!(f, "cannot handle entries with type code {}", type_code),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::InvalidControlByte { .. } => None,
            Self::InvalidVersion { .. } => None,
            Self::InvalidDetectHead { .. } => None,
            Self::InvalidDetectTail { .. } => None,
            Self::InvalidEncodeUtf8 { .. } => None,
            Self::InvalidEncodeInteger { .. } => None,
            Self::EncodeValueMismatch { .. } => None,
            Self::UnknownEntryType { .. } => None,
        }
    }
}
impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self { Self::Io(value) }
}


#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileHeader {
    pub file_type: SmallVec<[u8; 60]>,
    pub user_title: SmallVec<[u8; 60]>,
    pub version: u32,
    pub root_dir_position: u32,
    pub root_dir_size: u32,
    pub root_dir_time: u32,
    pub next_write_pos: u32,
    pub time: u32,
    pub largest_key_ary: u32,
    pub largest_dir_name_size: u32,
    pub largest_rez_name_size: u32,
    pub largest_comment_size: u32,
    pub is_sorted: bool,
}
impl FileHeader {
    pub fn try_read<R: Read>(r: &mut R) -> Result<Self, Error> {
        let mut one_buf = [0u8];
        let mut two_buf = [0u8; 2];
        let mut three_buf = [0u8; 3];

        r.read_exact(&mut two_buf)?;
        Self::check_crlf(two_buf[0], 0, b'\r', b'&')?;
        Self::check_crlf(two_buf[1], 1, b'\n', b'#')?;

        let mut file_type_buf = [0u8; 60];
        r.read_exact(&mut file_type_buf)?;
        let mut file_type = SmallVec::from_buf(file_type_buf);
        Self::strip_trailing_spaces(&mut file_type);

        r.read_exact(&mut two_buf)?;
        Self::check_crlf(two_buf[0], 2, b'\r', b'!')?;
        Self::check_crlf(two_buf[1], 3, b'\n', b'"')?;

        let mut user_title_buf = [0u8; 60];
        r.read_exact(&mut user_title_buf)?;
        let mut user_title = SmallVec::from_buf(user_title_buf);
        Self::strip_trailing_spaces(&mut user_title);

        r.read_exact(&mut three_buf)?;
        Self::check_crlf(three_buf[0], 4, b'\r', b'%')?;
        Self::check_crlf(three_buf[1], 5, b'\n', b'\'')?;
        Self::check_crlf(three_buf[2], 6, 0x1A, b'*')?;

        // the EOF character is a hint at the version
        let mut version = 0;
        if three_buf[2] == 0x1A {
            // assume version 1
            version = r.read_u32_le()?;
            if version != 1 {
                // fallback to format 2:
                // EOF ??? ??? ??? ??? | ??? ??? ??? vvv vvv vvv vvv
                //         we are here ^

                r.read_exact(&mut three_buf)?; // throw away
                version = r.read_u32_le()?;
                if version != 2 {
                    return Err(Error::InvalidVersion { expected: 2, obtained: version });
                }
            }
        } else if three_buf[2] == 0x2A {
            let mut two_buf = [0u8; 2];
            let mut encode_buf = [0u8; 32];
            let mut detect_encode_buf = [0u8; 32];

            r.read_exact(&mut one_buf)?;
            let head = one_buf[0];
            r.read_exact(&mut encode_buf)?;
            r.read_exact(&mut two_buf)?;
            let [tail, detect_head] = two_buf;

            if detect_head != (head ^ HEAD_TAIL_XOR) {
                return Err(Error::InvalidDetectHead { head, detect_head });
            }

            let encode_bytes = without_trailing_zero_bytes(&encode_buf);
            let encode_str = std::str::from_utf8(&encode_bytes)
                .map_err(|_| Error::InvalidEncodeUtf8 { detection_value: false, bytes: encode_bytes.to_vec() })?;
            let encode_value: u32 = encode_str.parse()
                .map_err(|_| Error::InvalidEncodeInteger { detection_value: false, string: encode_str.to_owned() })?;

            // magic number!
            let expected_encode_value = encode_value ^ ENCODE_VALUE_XOR;

            r.read_exact(&mut detect_encode_buf)?;

            let detect_encode_bytes = without_trailing_zero_bytes(&detect_encode_buf);
            let detect_encode_str = std::str::from_utf8(&detect_encode_bytes)
                .map_err(|_| Error::InvalidEncodeUtf8 { detection_value: true, bytes: detect_encode_bytes.to_vec() })?;
            let detect_encode_value: u32 = detect_encode_str.parse()
                .map_err(|_| Error::InvalidEncodeInteger { detection_value: true, string: detect_encode_str.to_owned() })?;

            if detect_encode_value != expected_encode_value {
                return Err(Error::EncodeValueMismatch { encode_value, detect_encode_value });
            }

            r.read_exact(&mut one_buf)?;
            let detect_tail = one_buf[0];

            if detect_tail != (tail ^ HEAD_TAIL_XOR) {
                return Err(Error::InvalidDetectTail { tail, detect_tail });
            }

            version = r.read_u32_le()?;
            if version != 1 {
                return Err(Error::InvalidVersion { expected: 1, obtained: version });
            }
        }

        let root_dir_pos = r.read_u32_le()?;
        let root_dir_size = r.read_u32_le()?;
        let root_dir_time = r.read_u32_le()?;
        let next_write_pos = r.read_u32_le()?;
        let time = r.read_u32_le()?;
        let largest_key_ary = r.read_u32_le()?;
        let largest_dir_name_size = r.read_u32_le()?;
        let largest_rez_name_size = r.read_u32_le()?;
        let largest_comment_size = r.read_u32_le()?;

        r.read_exact(&mut one_buf)?;
        let is_sorted = one_buf[0] != 0x00;

        Ok(Self {
            file_type,
            user_title,
            version,
            root_dir_position: root_dir_pos,
            root_dir_size,
            root_dir_time,
            next_write_pos,
            time,
            largest_key_ary,
            largest_dir_name_size,
            largest_rez_name_size,
            largest_comment_size,
            is_sorted,
        })
    }

    fn strip_trailing_spaces(value: &mut SmallVec<[u8; 60]>) {
        while let Some(b' ') = value.last() {
            value.pop();
        }
    }

    fn check_crlf(value: u8, index: usize, expected1: u8, expected2: u8) -> Result<(), Error> {
        if value == expected1 || value == expected2 {
            Ok(())
        } else {
            Err(Error::InvalidControlByte { index, expected_one_of: [expected1, expected2], obtained: value })
        }
    }
}


#[derive(Clone, Copy, Debug, Default)]
#[from_to_other(base_type = u32, derive_compare = "as_int")]
pub enum EntryType {
    #[default] Resource = 0,
    Directory = 1,
    Other(u32),
}


#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntryHeader {
    pub entry_type: EntryType,
    pub position: u32,
    pub size: u32,
    pub time: u32,
}
impl EntryHeader {
    pub fn try_read_next<R: Read>(r: &mut R) -> Result<Option<Self>, Error> {
        let entry_type_number = match r.read_u32_le() {
            Ok(etn) => etn,
            Err(e) => {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    return Ok(None);
                } else {
                    return Err(e.into());
                }
            },
        };
        let entry_type = entry_type_number.into();
        let position = r.read_u32_le()?;
        let size = r.read_u32_le()?;
        let time = r.read_u32_le()?;

        Ok(Some(Self {
            entry_type,
            position,
            size,
            time,
        }))
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Entry {
    Resource(Resource),
    Directory(Directory),
}
impl Entry {
    pub fn name(&self) -> Cow<str> {
        match self {
            Self::Resource(res) => Cow::Owned(format!("{}.{}", res.name.as_str(), res.extension.as_str())),
            Self::Directory(dir) => Cow::Borrowed(dir.name.as_str()),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Resource {
    pub header: EntryHeader,
    pub id: u32,
    pub extension: String,
    pub name: String,
    pub description: String,
    pub keys: Vec<u32>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Directory {
    pub header: EntryHeader,
    pub name: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct File {
    pub file_type: String,
    pub user_title: String,
    pub version: u32,
    pub time: u32,
    pub root_entries: Vec<Entry>,
}
impl File {
    pub fn try_read<R: Read + Seek>(r: &mut R) -> Result<Self, Error> {
        let header = FileHeader::try_read(r)?;
        let root_entries = read_directory_entries_recursive(r, header.root_dir_position, header.root_dir_size)?;
        let file_type = iso88591_bytes_to_string(&header.file_type);
        let user_title = iso88591_bytes_to_string(&header.user_title);

        Ok(File {
            file_type,
            user_title,
            version: header.version,
            time: header.time,
            root_entries,
        })
    }
}

fn iso88591_bytes_to_string(bytes: &[u8]) -> String {
    let string_byte_count: usize = bytes.iter()
        .map(|b| if *b >= 0x80 { 2 } else { 1 })
        .sum();
    let mut ret = String::with_capacity(string_byte_count);
    for &b in bytes {
        ret.push(char::from_u32(b as u32).unwrap());
    }
    ret
}

fn read_directory_entries_recursive<R: Read + Seek>(reader: &mut R, position: u32, length: u32) -> Result<Vec<Entry>, Error> {
    let length_usize: usize = length.try_into().unwrap();
    let mut buf = vec![0u8; length_usize];
    let mut entries = Vec::new();

    reader.seek(SeekFrom::Start(position.into()))?;
    reader.read_exact(&mut buf)?;
    let mut buf_reader = Cursor::new(buf.as_slice());

    while let Some(header) = EntryHeader::try_read_next(&mut buf_reader)? {
        match header.entry_type {
            EntryType::Directory => {
                let name_bytes = buf_reader.read_nul_terminated_byte_string()?;
                let name = iso88591_bytes_to_string(&name_bytes);

                let position = reader.stream_position()?;
                let sub_entries = read_directory_entries_recursive(reader, header.position, header.size)?;
                reader.seek(SeekFrom::Start(position))?;

                let directory = Directory {
                    header,
                    name,
                    entries: sub_entries,
                };
                entries.push(Entry::Directory(directory));
            },
            EntryType::Resource => {
                let id = buf_reader.read_u32_le()?;

                let mut extension_bytes = [0u8; 4];
                buf_reader.read_exact(&mut extension_bytes)?;
                extension_bytes.reverse();
                let extension_slice = without_leading_zero_bytes(&extension_bytes);
                let extension = iso88591_bytes_to_string(&extension_slice);

                let num_keys = buf_reader.read_u32_le()?;

                let name_bytes = buf_reader.read_nul_terminated_byte_string()?;
                let name = iso88591_bytes_to_string(&name_bytes);

                let description_bytes = buf_reader.read_nul_terminated_byte_string()?;
                let description = iso88591_bytes_to_string(&description_bytes);

                let mut keys = Vec::with_capacity(num_keys.try_into().unwrap());
                for _ in 0..num_keys {
                    let key = buf_reader.read_u32_le()?;
                    keys.push(key);
                }

                let resource = Resource {
                    header,
                    id,
                    extension,
                    name,
                    description,
                    keys,
                };
                entries.push(Entry::Resource(resource));
            },
            EntryType::Other(type_code) => return Err(Error::UnknownEntryType { type_code }),
        }
    }

    Ok(entries)
}


fn without_trailing_zero_bytes(value: &[u8]) -> &[u8] {
    let mut ret = value;
    while ret.len() > 0 && ret[ret.len() - 1] == 0x00 {
        ret = &ret[..ret.len() - 1];
    }
    ret
}

fn without_leading_zero_bytes(value: &[u8]) -> &[u8] {
    let mut ret = value;
    while ret.len() > 0 && ret[0] == 0x00 {
        ret = &ret[1..];
    }
    ret
}
