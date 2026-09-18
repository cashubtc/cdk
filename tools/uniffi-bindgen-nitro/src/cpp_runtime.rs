//! The buffer reader/writer the generated bridge uses.
//!
//! It is emitted rather than vendored so the generated tree stays the only
//! thing a project has to build, and so the `RustBuffer` free symbol is the one
//! the metadata actually names.

/// The runtime helpers, emitted into the bridge translation unit.
pub fn helpers(rustbuffer_free: &str, rustbuffer_from_bytes: &str) -> String {
    format!(
        r#"using ffi::RustBuffer;
using ffi::RustCallStatus;

/// Big-endian reader over the UniFFI buffer format.
class BufferReader final {{
public:
  BufferReader(const uint8_t* data, size_t size) : data_(data), size_(size) {{}}

  int8_t readI8() {{ return static_cast<int8_t>(readByte()); }}
  uint8_t readU8() {{ return readByte(); }}
  bool readBool() {{ return readByte() != 0; }}
  int16_t readI16() {{ return static_cast<int16_t>(readU16()); }}
  uint16_t readU16() {{ return static_cast<uint16_t>(readUInt(2)); }}
  int32_t readI32() {{ return static_cast<int32_t>(readU32()); }}
  uint32_t readU32() {{ return static_cast<uint32_t>(readUInt(4)); }}
  int64_t readI64() {{ return static_cast<int64_t>(readU64()); }}
  uint64_t readU64() {{ return readUInt(8); }}

  float readF32() {{
    uint32_t bits = readU32();
    float value;
    std::memcpy(&value, &bits, sizeof(value));
    return value;
  }}

  double readF64() {{
    uint64_t bits = readU64();
    double value;
    std::memcpy(&value, &bits, sizeof(value));
    return value;
  }}

  std::vector<uint8_t> readBytes() {{
    size_t length = readLength();
    require(length);
    std::vector<uint8_t> value(data_ + position_, data_ + position_ + length);
    position_ += length;
    return value;
  }}

  std::string readString() {{
    size_t length = readLength();
    require(length);
    std::string value(reinterpret_cast<const char*>(data_ + position_), length);
    position_ += length;
    return value;
  }}

  /// Number of items or bytes that follow, as UniFFI writes it: an i32.
  size_t readLength() {{
    int32_t length = readI32();
    if (length < 0) {{
      throw std::runtime_error("uniffi buffer declares a negative length");
    }}
    return static_cast<size_t>(length);
  }}

  void finish() const {{
    if (position_ != size_) {{
      throw std::runtime_error("uniffi buffer has trailing bytes");
    }}
  }}

private:
  uint8_t readByte() {{
    require(1);
    return data_[position_++];
  }}

  uint64_t readUInt(size_t width) {{
    require(width);
    uint64_t value = 0;
    for (size_t i = 0; i < width; i++) {{
      value = (value << 8) | data_[position_ + i];
    }}
    position_ += width;
    return value;
  }}

  void require(size_t count) const {{
    if (position_ + count > size_) {{
      throw std::runtime_error("uniffi buffer ended early");
    }}
  }}

  const uint8_t* data_;
  size_t size_;
  size_t position_ = 0;
}};

/// Big-endian writer producing the UniFFI buffer format.
class BufferWriter final {{
public:
  void writeI8(int8_t value) {{ bytes_.push_back(static_cast<uint8_t>(value)); }}
  void writeU8(uint8_t value) {{ bytes_.push_back(value); }}
  void writeBool(bool value) {{ writeI8(value ? 1 : 0); }}
  void writeI16(int16_t value) {{ writeUInt(static_cast<uint16_t>(value), 2); }}
  void writeU16(uint16_t value) {{ writeUInt(value, 2); }}
  void writeI32(int32_t value) {{ writeUInt(static_cast<uint32_t>(value), 4); }}
  void writeU32(uint32_t value) {{ writeUInt(value, 4); }}
  void writeI64(int64_t value) {{ writeUInt(static_cast<uint64_t>(value), 8); }}
  void writeU64(uint64_t value) {{ writeUInt(value, 8); }}

  void writeF32(float value) {{
    uint32_t bits;
    std::memcpy(&bits, &value, sizeof(bits));
    writeU32(bits);
  }}

  void writeF64(double value) {{
    uint64_t bits;
    std::memcpy(&bits, &value, sizeof(bits));
    writeU64(bits);
  }}

  void writeBytes(const std::vector<uint8_t>& value) {{
    writeLength(value.size());
    bytes_.insert(bytes_.end(), value.begin(), value.end());
  }}

  void writeString(const std::string& value) {{
    writeLength(value.size());
    bytes_.insert(bytes_.end(), value.begin(), value.end());
  }}

  void writeLength(size_t count) {{
    if (count > static_cast<size_t>(INT32_MAX)) {{
      throw std::runtime_error("value is too large for the uniffi buffer format");
    }}
    writeI32(static_cast<int32_t>(count));
  }}

  const std::vector<uint8_t>& bytes() const {{ return bytes_; }}

private:
  void writeUInt(uint64_t value, size_t width) {{
    for (size_t i = width; i > 0; i--) {{
      bytes_.push_back(static_cast<uint8_t>((value >> ((i - 1) * 8)) & 0xff));
    }}
  }}

  std::vector<uint8_t> bytes_;
}};

RustBuffer emptyBuffer() {{
  RustBuffer buffer;
  buffer.capacity = 0;
  buffer.len = 0;
  buffer.data = nullptr;
  return buffer;
}}

void freeBuffer(RustBuffer buffer) {{
  if (buffer.data == nullptr && buffer.capacity == 0) {{
    return;
  }}
  RustCallStatus status;
  status.code = 0;
  status.errorBuf = emptyBuffer();
  ffi::{rustbuffer_free}(buffer, &status);
}}

/// Copy a Rust-owned buffer into C++ memory and hand the allocation back.
///
/// The copy is unavoidable: the bytes belong to Rust's allocator, so they
/// cannot outlive the call without being duplicated.
std::vector<uint8_t> consumeBuffer(RustBuffer buffer) {{
  std::vector<uint8_t> bytes;
  if (buffer.data != nullptr && buffer.len > 0) {{
    bytes.assign(buffer.data, buffer.data + buffer.len);
  }}
  freeBuffer(buffer);
  return bytes;
}}

std::string consumeBufferAsString(RustBuffer buffer) {{
  std::string text;
  if (buffer.data != nullptr && buffer.len > 0) {{
    text.assign(reinterpret_cast<const char*>(buffer.data), buffer.len);
  }}
  freeBuffer(buffer);
  return text;
}}

/// Hand foreign-owned bytes to Rust as a `RustBuffer` it takes ownership of.
RustBuffer bytesToBuffer(const std::vector<uint8_t>& bytes) {{
  ffi::ForeignBytes borrowed;
  borrowed.len = static_cast<int32_t>(bytes.size());
  borrowed.data = bytes.data();
  RustCallStatus status;
  status.code = 0;
  status.errorBuf = emptyBuffer();
  RustBuffer buffer = ffi::{from_bytes}(borrowed, &status);
  if (status.code != ffi::RUST_CALL_SUCCESS) {{
    freeBuffer(status.errorBuf);
    throw std::runtime_error("uniffi could not allocate a buffer");
  }}
  return buffer;
}}

RustBuffer stringToBuffer(const std::string& text) {{
  std::vector<uint8_t> bytes(text.begin(), text.end());
  return bytesToBuffer(bytes);
}}

std::string jsonEscape(const std::string& text) {{
  std::string escaped;
  escaped.reserve(text.size() + 2);
  for (unsigned char character : text) {{
    switch (character) {{
      case '"': escaped += "\\\""; break;
      case '\\': escaped += "\\\\"; break;
      case '\n': escaped += "\\n"; break;
      case '\r': escaped += "\\r"; break;
      case '\t': escaped += "\\t"; break;
      default:
        if (character < 0x20) {{
          char slot[7];
          std::snprintf(slot, sizeof(slot), "\\u%04x", character);
          escaped += slot;
        }} else {{
          escaped += static_cast<char>(character);
        }}
    }}
  }}
  return escaped;
}}

std::string toHexString(const std::vector<uint8_t>& bytes) {{
  static const char* digits = "0123456789abcdef";
  std::string hex;
  hex.reserve(bytes.size() * 2);
  for (uint8_t byte : bytes) {{
    hex.push_back(digits[byte >> 4]);
    hex.push_back(digits[byte & 0x0f]);
  }}
  return hex;
}}

/// Turn a panic or a cancellation into a C++ exception.
///
/// An expected `Result::Err` is left alone: only the caller knows which error
/// enum to decode it as.
void checkUnexpected(RustCallStatus& status) {{
  if (status.code == ffi::RUST_CALL_UNEXPECTED_ERROR) {{
    std::string message = consumeBufferAsString(status.errorBuf);
    status.errorBuf = emptyBuffer();
    throw std::runtime_error("rust panicked: " + message);
  }}
  if (status.code == ffi::RUST_CALL_CANCELLED) {{
    throw std::runtime_error("rust call was cancelled");
  }}
}}
"#,
        rustbuffer_free = rustbuffer_free,
        from_bytes = rustbuffer_from_bytes,
    )
}
