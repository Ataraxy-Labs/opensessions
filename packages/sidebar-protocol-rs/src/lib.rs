const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShimToServer {
    Hello(ShimHello),
    Key(KeyMessage),
    Resize { width: u16, height: u16 },
    Mouse(MouseMessage),
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShimHello {
    pub protocol: u16,
    pub pane_id: String,
    pub session_name: String,
    pub window_id: Option<String>,
    pub client_tty: Option<String>,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMessage {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Up,
    Down,
    Tab,
    Enter,
    Esc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers(u8);

impl KeyModifiers {
    pub const SHIFT: Self = Self(1 << 0);
    pub const ALT: Self = Self(1 << 1);
    pub const CONTROL: Self = Self(1 << 2);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for KeyModifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseMessage {
    pub kind: MouseEventKind,
    pub button: MouseButton,
    pub column: u16,
    pub row: u16,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Down,
    Up,
    Drag,
    Move,
    ScrollUp,
    ScrollDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerToShim {
    Hello {
        protocol: u16,
    },
    FullFrame {
        seq: u32,
        width: u16,
        height: u16,
        rows: Vec<Vec<u8>>,
    },
    PatchFrame {
        seq: u32,
        width: u16,
        height: u16,
        changed_rows: Vec<(u16, Vec<u8>)>,
        clear_from_row: Option<u16>,
    },
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    FrameTooShort,
    FrameTooLarge,
    LengthMismatch,
    UnknownMessageType(u8),
    InvalidPayload,
    InvalidUtf8,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ProtocolError {}

pub fn encode_shim_message(message: &ShimToServer) -> Vec<u8> {
    let mut payload = Vec::new();
    match message {
        ShimToServer::Hello(hello) => {
            payload.push(1);
            put_u16(&mut payload, hello.protocol);
            put_u16(&mut payload, hello.width);
            put_u16(&mut payload, hello.height);
            put_string(&mut payload, &hello.pane_id);
            put_string(&mut payload, &hello.session_name);
            put_option_string(&mut payload, hello.window_id.as_deref());
            put_option_string(&mut payload, hello.client_tty.as_deref());
        }
        ShimToServer::Key(key) => {
            payload.push(2);
            put_key_code(&mut payload, &key.code);
            payload.push(key.modifiers.bits());
        }
        ShimToServer::Resize { width, height } => {
            payload.push(3);
            put_u16(&mut payload, *width);
            put_u16(&mut payload, *height);
        }
        ShimToServer::Mouse(mouse) => {
            payload.push(4);
            payload.push(mouse_kind_byte(mouse.kind));
            payload.push(mouse_button_byte(mouse.button));
            put_u16(&mut payload, mouse.column);
            put_u16(&mut payload, mouse.row);
            payload.push(mouse.modifiers.bits());
        }
        ShimToServer::Close => payload.push(5),
    }
    envelope(payload)
}

pub fn decode_shim_message(frame: &[u8]) -> Result<ShimToServer, ProtocolError> {
    let payload = payload(frame)?;
    let (&message_type, payload) = payload.split_first().ok_or(ProtocolError::FrameTooShort)?;
    let mut cursor = Cursor::new(payload);
    match message_type {
        1 => Ok(ShimToServer::Hello(ShimHello {
            protocol: cursor.u16()?,
            width: cursor.u16()?,
            height: cursor.u16()?,
            pane_id: cursor.string()?,
            session_name: cursor.string()?,
            window_id: cursor.option_string()?,
            client_tty: cursor.option_string()?,
        })),
        2 => Ok(ShimToServer::Key(KeyMessage {
            code: cursor.key_code()?,
            modifiers: KeyModifiers(cursor.u8()?),
        })),
        3 => Ok(ShimToServer::Resize {
            width: cursor.u16()?,
            height: cursor.u16()?,
        }),
        4 => Ok(ShimToServer::Mouse(MouseMessage {
            kind: mouse_kind(cursor.u8()?)?,
            button: mouse_button(cursor.u8()?)?,
            column: cursor.u16()?,
            row: cursor.u16()?,
            modifiers: KeyModifiers(cursor.u8()?),
        })),
        5 => Ok(ShimToServer::Close),
        other => Err(ProtocolError::UnknownMessageType(other)),
    }
}

pub fn encode_server_message(message: &ServerToShim) -> Vec<u8> {
    let mut payload = Vec::new();
    match message {
        ServerToShim::Hello { protocol } => {
            payload.push(101);
            put_u16(&mut payload, *protocol);
        }
        ServerToShim::FullFrame {
            seq,
            width,
            height,
            rows,
        } => {
            payload.push(102);
            put_u32(&mut payload, *seq);
            put_u16(&mut payload, *width);
            put_u16(&mut payload, *height);
            put_bytes_vec(&mut payload, rows);
        }
        ServerToShim::PatchFrame {
            seq,
            width,
            height,
            changed_rows,
            clear_from_row,
        } => {
            payload.push(103);
            put_u32(&mut payload, *seq);
            put_u16(&mut payload, *width);
            put_u16(&mut payload, *height);
            put_u32(&mut payload, changed_rows.len() as u32);
            for (row, bytes) in changed_rows {
                put_u16(&mut payload, *row);
                put_bytes(&mut payload, bytes);
            }
            match clear_from_row {
                Some(row) => {
                    payload.push(1);
                    put_u16(&mut payload, *row);
                }
                None => payload.push(0),
            }
        }
        ServerToShim::Quit => payload.push(104),
    }
    envelope(payload)
}

pub fn decode_server_message(frame: &[u8]) -> Result<ServerToShim, ProtocolError> {
    let payload = payload(frame)?;
    let (&message_type, payload) = payload.split_first().ok_or(ProtocolError::FrameTooShort)?;
    let mut cursor = Cursor::new(payload);
    match message_type {
        101 => Ok(ServerToShim::Hello {
            protocol: cursor.u16()?,
        }),
        102 => Ok(ServerToShim::FullFrame {
            seq: cursor.u32()?,
            width: cursor.u16()?,
            height: cursor.u16()?,
            rows: cursor.bytes_vec()?,
        }),
        103 => {
            let seq = cursor.u32()?;
            let width = cursor.u16()?;
            let height = cursor.u16()?;
            let len = cursor.u32()? as usize;
            let mut changed_rows = Vec::with_capacity(len);
            for _ in 0..len {
                changed_rows.push((cursor.u16()?, cursor.bytes()?));
            }
            let clear_from_row = match cursor.u8()? {
                0 => None,
                1 => Some(cursor.u16()?),
                _ => return Err(ProtocolError::InvalidPayload),
            };
            Ok(ServerToShim::PatchFrame {
                seq,
                width,
                height,
                changed_rows,
                clear_from_row,
            })
        }
        104 => Ok(ServerToShim::Quit),
        other => Err(ProtocolError::UnknownMessageType(other)),
    }
}

fn envelope(payload: Vec<u8>) -> Vec<u8> {
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    frame
}

fn payload(frame: &[u8]) -> Result<&[u8], ProtocolError> {
    if frame.len() < 4 {
        return Err(ProtocolError::FrameTooShort);
    }
    let len = u32::from_le_bytes(frame[..4].try_into().expect("slice has four bytes")) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    if frame.len() != 4 + len {
        return Err(ProtocolError::LengthMismatch);
    }
    Ok(&frame[4..])
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_string(out: &mut Vec<u8>, value: &str) {
    put_bytes(out, value.as_bytes());
}

fn put_option_string(out: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            out.push(1);
            put_string(out, value);
        }
        None => out.push(0),
    }
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn put_bytes_vec(out: &mut Vec<u8>, rows: &[Vec<u8>]) {
    put_u32(out, rows.len() as u32);
    for row in rows {
        put_bytes(out, row);
    }
}

fn put_key_code(out: &mut Vec<u8>, code: &KeyCode) {
    match code {
        KeyCode::Char(ch) => {
            out.push(1);
            put_u32(out, *ch as u32);
        }
        KeyCode::Up => out.push(2),
        KeyCode::Down => out.push(3),
        KeyCode::Tab => out.push(4),
        KeyCode::Enter => out.push(5),
        KeyCode::Esc => out.push(6),
    }
}

fn mouse_kind_byte(kind: MouseEventKind) -> u8 {
    match kind {
        MouseEventKind::Down => 1,
        MouseEventKind::Up => 2,
        MouseEventKind::Drag => 3,
        MouseEventKind::Move => 4,
        MouseEventKind::ScrollUp => 5,
        MouseEventKind::ScrollDown => 6,
    }
}

fn mouse_kind(byte: u8) -> Result<MouseEventKind, ProtocolError> {
    match byte {
        1 => Ok(MouseEventKind::Down),
        2 => Ok(MouseEventKind::Up),
        3 => Ok(MouseEventKind::Drag),
        4 => Ok(MouseEventKind::Move),
        5 => Ok(MouseEventKind::ScrollUp),
        6 => Ok(MouseEventKind::ScrollDown),
        _ => Err(ProtocolError::InvalidPayload),
    }
}

fn mouse_button_byte(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::None => 4,
    }
}

fn mouse_button(byte: u8) -> Result<MouseButton, ProtocolError> {
    match byte {
        1 => Ok(MouseButton::Left),
        2 => Ok(MouseButton::Middle),
        3 => Ok(MouseButton::Right),
        4 => Ok(MouseButton::None),
        _ => Err(ProtocolError::InvalidPayload),
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        let byte = *self
            .bytes
            .get(self.offset)
            .ok_or(ProtocolError::InvalidPayload)?;
        self.offset += 1;
        Ok(byte)
    }

    fn u16(&mut self) -> Result<u16, ProtocolError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes(
            bytes.try_into().expect("slice has two bytes"),
        ))
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(
            bytes.try_into().expect("slice has four bytes"),
        ))
    }

    fn bytes(&mut self) -> Result<Vec<u8>, ProtocolError> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    fn bytes_vec(&mut self) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let len = self.u32()? as usize;
        let mut rows = Vec::with_capacity(len);
        for _ in 0..len {
            rows.push(self.bytes()?);
        }
        Ok(rows)
    }

    fn string(&mut self) -> Result<String, ProtocolError> {
        String::from_utf8(self.bytes()?).map_err(|_| ProtocolError::InvalidUtf8)
    }

    fn option_string(&mut self) -> Result<Option<String>, ProtocolError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.string()?)),
            _ => Err(ProtocolError::InvalidPayload),
        }
    }

    fn key_code(&mut self) -> Result<KeyCode, ProtocolError> {
        match self.u8()? {
            1 => char::from_u32(self.u32()?)
                .map(KeyCode::Char)
                .ok_or(ProtocolError::InvalidPayload),
            2 => Ok(KeyCode::Up),
            3 => Ok(KeyCode::Down),
            4 => Ok(KeyCode::Tab),
            5 => Ok(KeyCode::Enter),
            6 => Ok(KeyCode::Esc),
            _ => Err(ProtocolError::InvalidPayload),
        }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ProtocolError::InvalidPayload)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(ProtocolError::InvalidPayload)?;
        self.offset = end;
        Ok(bytes)
    }
}
