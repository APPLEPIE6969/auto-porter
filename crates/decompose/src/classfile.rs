//! Minimal Java class-file parser (JVMS chapter 4).
//!
//! Phase 1 uses this instead of `smali`/`jadx` (Android/Dalvik tools) and
//! instead of `javap` (debugging only). It accepts any class-file version
//! because unknown attributes are skipped by length; only structure,
//! annotations and constant-pool refs are needed.

#[derive(Debug, Clone)]
pub enum Primitive {
    Int(i64),
    Float(f64),
    Utf8(String),
}

#[derive(Debug, Clone)]
pub enum AnnValue {
    Prim(Primitive),
    Boolean(bool),
    Enum {
        type_name: String,
        const_name: String,
    },
    /// Class element: the raw descriptor, e.g. `Lnet/minecraft/world/level/Block;`
    Class(String),
    Nested(Annotation),
    Array(Vec<AnnValue>),
}

#[derive(Debug, Clone)]
pub struct Annotation {
    /// Internal name, e.g. `org/spongepowered/asm/mixin/Mixin`.
    pub type_name: String,
    pub values: Vec<(String, AnnValue)>,
}

impl Annotation {
    pub fn value(&self, key: &str) -> Option<&AnnValue> {
        self.values.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// First `Class` element of an `Array`/single `Class` value, internal name.
    pub fn class_value(&self, key: &str) -> Option<String> {
        match self.value(key) {
            Some(AnnValue::Class(desc)) => Some(descriptor_to_internal(desc)),
            Some(AnnValue::Array(vals)) => vals.first().map(|v| match v {
                AnnValue::Class(desc) => descriptor_to_internal(desc),
                _ => String::new(),
            }),
            _ => None,
        }
    }

    pub fn class_values(&self, key: &str) -> Vec<String> {
        match self.value(key) {
            Some(AnnValue::Class(desc)) => vec![descriptor_to_internal(desc)],
            Some(AnnValue::Array(vals)) => vals
                .iter()
                .filter_map(|v| match v {
                    AnnValue::Class(desc) => Some(descriptor_to_internal(desc)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn string_value(&self, key: &str) -> Option<String> {
        match self.value(key) {
            Some(AnnValue::Prim(Primitive::Utf8(s))) => Some(s.clone()),
            Some(AnnValue::Array(vals)) => vals.first().map(|v| match v {
                AnnValue::Prim(Primitive::Utf8(s)) => s.clone(),
                _ => String::new(),
            }),
            _ => None,
        }
    }

    pub fn bool_value(&self, key: &str) -> Option<bool> {
        match self.value(key) {
            Some(AnnValue::Boolean(b)) => Some(*b),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawField {
    pub access: u16,
    pub name: String,
    pub descriptor: String,
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone)]
pub struct RawMethod {
    pub access: u16,
    pub name: String,
    pub descriptor: String,
    pub has_code: bool,
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone, Default)]
pub struct Constants {
    /// (owner, name, descriptor) — Fieldref entries.
    pub field_refs: Vec<(String, String, String)>,
    /// (owner, name, descriptor) — Methodref and InterfaceMethodref entries.
    pub method_refs: Vec<(String, String, String)>,
    pub class_refs: Vec<String>,
    pub string_consts: Vec<String>,
}

impl Constants {
    pub fn has_field_ref(&self, name: &str, owner_contains: &str) -> bool {
        self.field_refs
            .iter()
            .any(|(owner, n, _)| n == name && owner.contains(owner_contains))
    }

    pub fn has_method_ref(&self, name: &str, owner_contains: &str) -> bool {
        self.method_refs
            .iter()
            .any(|(owner, n, _)| n == name && owner.contains(owner_contains))
    }
}

#[derive(Debug, Clone)]
pub struct RawClass {
    pub major: u16,
    pub minor: u16,
    pub access: u16,
    pub this_class: String,
    pub super_class: Option<String>,
    pub interfaces: Vec<String>,
    pub fields: Vec<RawField>,
    pub methods: Vec<RawMethod>,
    pub class_annotations: Vec<Annotation>,
    pub constants: Constants,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    fn u1(&mut self) -> Result<u8, String> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or("unexpected end of class file (u1)")?;
        self.pos += 1;
        Ok(b)
    }
    fn u2(&mut self) -> Result<u16, String> {
        let b = self
            .data
            .get(self.pos..self.pos + 2)
            .ok_or("unexpected end of class file (u2)")?;
        self.pos += 2;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u4(&mut self) -> Result<u32, String> {
        let b = self
            .data
            .get(self.pos..self.pos + 4)
            .ok_or("unexpected end of class file (u4)")?;
        self.pos += 4;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let b = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or("unexpected end of class file (bytes)")?;
        self.pos += n;
        Ok(b)
    }
}

#[allow(dead_code)]
enum Cp {
    Utf8(String),
    Int(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Class(u16),
    Str(u16),
    FieldRef(u16, u16),
    MethodRef(u16, u16),
    IfaceRef(u16, u16),
    NameAndType(u16, u16),
    MethodHandle(u16),
    MethodType(u16),
    Dynamic(u16, u16),
    InvokeDynamic(u16, u16),
    Module(u16),
    Package(u16),
    Phantom, // second slot of Long/Double
}

pub fn parse(bytes: &[u8]) -> Result<RawClass, String> {
    let mut r = Reader::new(bytes);
    let magic = r.u4()?;
    if magic != 0xCAFEBABE {
        return Err(format!("not a class file (magic 0x{magic:08x})"));
    }
    let minor = r.u2()?;
    let major = r.u2()?;

    // ---- constant pool
    let cp_count = r.u2()? as usize;
    let mut cp: Vec<Cp> = Vec::with_capacity(cp_count);
    cp.push(Cp::Phantom); // index 0 unused
    let mut i = 1;
    while i < cp_count {
        let tag = r.u1()?;
        let entry = match tag {
            1 => {
                let len = r.u2()? as usize;
                let b = r.bytes(len)?;
                Cp::Utf8(String::from_utf8_lossy(b).into_owned())
            }
            3 => Cp::Int(i32::from_be_bytes(r.bytes(4)?.try_into().unwrap())),
            4 => Cp::Float(f32::from_be_bytes(r.bytes(4)?.try_into().unwrap())),
            5 => {
                let v = i64::from_be_bytes(r.bytes(8)?.try_into().unwrap());
                cp.push(Cp::Phantom);
                i += 1;
                Cp::Long(v)
            }
            6 => {
                let v = f64::from_be_bytes(r.bytes(8)?.try_into().unwrap());
                cp.push(Cp::Phantom);
                i += 1;
                Cp::Double(v)
            }
            7 => Cp::Class(r.u2()?),
            8 => Cp::Str(r.u2()?),
            9 => {
                let a = r.u2()?;
                let b = r.u2()?;
                Cp::FieldRef(a, b)
            }
            10 | 11 => {
                let a = r.u2()?;
                let b = r.u2()?;
                Cp::MethodRef(a, b)
            }
            12 => {
                let a = r.u2()?;
                let b = r.u2()?;
                Cp::NameAndType(a, b)
            }
            15 => {
                let _kind = r.u1()?;
                Cp::MethodHandle(r.u2()?)
            }
            16 => Cp::MethodType(r.u2()?),
            17 => {
                let a = r.u2()?;
                let b = r.u2()?;
                Cp::Dynamic(a, b)
            }
            18 => {
                let a = r.u2()?;
                let b = r.u2()?;
                Cp::InvokeDynamic(a, b)
            }
            19 => Cp::Module(r.u2()?),
            20 => Cp::Package(r.u2()?),
            other => return Err(format!("unknown constant pool tag {other}")),
        };
        cp.push(entry);
        i += 1;
    }

    let pool = Pool { cp };

    let access = r.u2()?;
    let this_class = pool.class_name(r.u2()?)?;
    let super_idx = r.u2()?;
    let super_class = if super_idx == 0 {
        None
    } else {
        Some(pool.class_name(super_idx)?)
    };

    let iface_count = r.u2()? as usize;
    let mut interfaces = Vec::with_capacity(iface_count);
    for _ in 0..iface_count {
        interfaces.push(pool.class_name(r.u2()?)?);
    }

    let mut constants = Constants::default();
    for entry in pool.cp.iter().skip(1) {
        let (a, b) = match entry {
            Cp::FieldRef(a, b) | Cp::MethodRef(a, b) | Cp::IfaceRef(a, b) => (*a, *b),
            Cp::Class(name_idx) => {
                if let Ok(name) = pool.utf8(*name_idx) {
                    constants.class_refs.push(name);
                }
                continue;
            }
            Cp::Str(s_idx) => {
                if let Ok(s) = pool.utf8(*s_idx) {
                    constants.string_consts.push(s);
                }
                continue;
            }
            _ => continue,
        };
        let (name_idx, desc_idx) = match (pool.cp.get(a as usize), pool.cp.get(b as usize)) {
            (Some(Cp::Class(_)), Some(Cp::NameAndType(n, d))) => (*n, *d),
            _ => continue,
        };
        let owner = pool.class_name(a)?;
        let name = pool.utf8(name_idx)?;
        let desc = pool.utf8(desc_idx)?;
        match entry {
            Cp::FieldRef(_, _) => constants.field_refs.push((owner, name, desc)),
            _ => constants.method_refs.push((owner, name, desc)),
        }
    }

    // ---- fields
    let field_count = r.u2()? as usize;
    let mut fields = Vec::with_capacity(field_count);
    for _ in 0..field_count {
        let f_access = r.u2()?;
        let f_name = pool.utf8(r.u2()?)?;
        let f_desc = pool.utf8(r.u2()?)?;
        let annotations = read_member_annotations(&mut r, &pool)?;
        fields.push(RawField {
            access: f_access,
            name: f_name,
            descriptor: f_desc,
            annotations,
        });
    }

    // ---- methods
    let method_count = r.u2()? as usize;
    let mut methods = Vec::with_capacity(method_count);
    for _ in 0..method_count {
        let m_access = r.u2()?;
        let m_name = pool.utf8(r.u2()?)?;
        let m_desc = pool.utf8(r.u2()?)?;
        let (annotations, has_code) = read_method_attributes(&mut r, &pool)?;
        methods.push(RawMethod {
            access: m_access,
            name: m_name,
            descriptor: m_desc,
            has_code,
            annotations,
        });
    }

    let class_annotations = read_class_annotations(&mut r, &pool)?;

    Ok(RawClass {
        major,
        minor,
        access,
        this_class,
        super_class,
        interfaces,
        fields,
        methods,
        class_annotations,
        constants,
    })
}

// --------------------------------------------------------------- attributes

const ANN_ATTRS: [&str; 2] = ["RuntimeVisibleAnnotations", "RuntimeInvisibleAnnotations"];

fn read_member_annotations(r: &mut Reader, pool: &Pool) -> Result<Vec<Annotation>, String> {
    let count = r.u2()? as usize;
    let mut anns = Vec::new();
    for _ in 0..count {
        let name = pool.utf8(r.u2()?)?;
        let len = r.u4()? as usize;
        let data = r.bytes(len)?;
        if ANN_ATTRS.contains(&name.as_str()) {
            anns.extend(decode_annotations(data, pool)?);
        }
    }
    Ok(anns)
}

fn read_method_attributes(r: &mut Reader, pool: &Pool) -> Result<(Vec<Annotation>, bool), String> {
    let count = r.u2()? as usize;
    let mut anns = Vec::new();
    let mut has_code = false;
    for _ in 0..count {
        let name = pool.utf8(r.u2()?)?;
        let len = r.u4()? as usize;
        let data = r.bytes(len)?;
        if ANN_ATTRS.contains(&name.as_str()) {
            anns.extend(decode_annotations(data, pool)?);
        } else if name == "Code" {
            has_code = true;
        }
    }
    Ok((anns, has_code))
}

fn read_class_annotations(r: &mut Reader, pool: &Pool) -> Result<Vec<Annotation>, String> {
    let count = r.u2()? as usize;
    let mut anns = Vec::new();
    for _ in 0..count {
        let name = pool.utf8(r.u2()?)?;
        let len = r.u4()? as usize;
        let data = r.bytes(len)?;
        if ANN_ATTRS.contains(&name.as_str()) {
            anns.extend(decode_annotations(data, pool)?);
        }
    }
    Ok(anns)
}

// --------------------------------------------------------------- annotations
// element_value entries reference the CLASS's constant pool by index, so the
// pool must be threaded through every read below (JVMS 4.7.16).

fn decode_annotations(data: &[u8], pool: &Pool) -> Result<Vec<Annotation>, String> {
    let mut r = Reader::new(data);
    let count = r.u2()? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_annotation(&mut r, pool)?);
    }
    Ok(out)
}

fn read_annotation(r: &mut Reader, pool: &Pool) -> Result<Annotation, String> {
    let type_idx = r.u2()?;
    let type_desc = pool.utf8(type_idx)?;
    let num = r.u2()? as usize;
    let mut values = Vec::with_capacity(num);
    for _ in 0..num {
        let name = pool.utf8(r.u2()?)?;
        let v = read_element_value(r, pool)?;
        values.push((name, v));
    }
    Ok(Annotation {
        type_name: descriptor_to_internal(&type_desc),
        values,
    })
}

fn read_element_value(r: &mut Reader, pool: &Pool) -> Result<AnnValue, String> {
    let tag = r.u1()?;
    Ok(match tag {
        b'Z' => AnnValue::Boolean(pool.integer(r.u2()?)? != 0),
        b'B' | b'S' | b'I' | b'J' => AnnValue::Prim(Primitive::Int(pool.integer(r.u2()?)?)),
        b'C' => AnnValue::Prim(Primitive::Int(pool.integer(r.u2()?)?)),
        b'F' | b'D' => AnnValue::Prim(Primitive::Float(pool.floating(r.u2()?)?)),
        b's' => AnnValue::Prim(Primitive::Utf8(pool.utf8(r.u2()?)?)),
        b'e' => {
            let t = pool.utf8(r.u2()?)?;
            let c = pool.utf8(r.u2()?)?;
            AnnValue::Enum {
                type_name: descriptor_to_internal(&t),
                const_name: c,
            }
        }
        b'c' => AnnValue::Class(pool.utf8(r.u2()?)?),
        b'@' => AnnValue::Nested(read_annotation(r, pool)?),
        b'[' => {
            let n = r.u2()? as usize;
            let mut vals = Vec::with_capacity(n);
            for _ in 0..n {
                vals.push(read_element_value(r, pool)?);
            }
            AnnValue::Array(vals)
        }
        other => return Err(format!("unknown element_value tag {other}")),
    })
}

pub fn descriptor_to_internal(desc: &str) -> String {
    desc.trim_start_matches('L')
        .trim_end_matches(';')
        .to_string()
}

// ------------------------------------------------------------ constant pool

struct Pool {
    cp: Vec<Cp>,
}

impl Pool {
    fn utf8(&self, idx: u16) -> Result<String, String> {
        match self.cp.get(idx as usize) {
            Some(Cp::Utf8(s)) => Ok(s.clone()),
            _ => Err(format!("constant pool index {idx} is not a Utf8 entry")),
        }
    }
    fn class_name(&self, idx: u16) -> Result<String, String> {
        match self.cp.get(idx as usize) {
            Some(Cp::Class(name_idx)) => self.utf8(*name_idx),
            _ => Err(format!("constant pool index {idx} is not a Class entry")),
        }
    }
    fn integer(&self, idx: u16) -> Result<i64, String> {
        match self.cp.get(idx as usize) {
            Some(Cp::Int(v)) => Ok(*v as i64),
            Some(Cp::Long(v)) => Ok(*v),
            _ => Err(format!(
                "constant pool index {idx} is not an int/long entry"
            )),
        }
    }
    fn floating(&self, idx: u16) -> Result<f64, String> {
        match self.cp.get(idx as usize) {
            Some(Cp::Float(v)) => Ok(*v as f64),
            Some(Cp::Double(v)) => Ok(*v),
            _ => Err(format!(
                "constant pool index {idx} is not a float/double entry"
            )),
        }
    }
}
