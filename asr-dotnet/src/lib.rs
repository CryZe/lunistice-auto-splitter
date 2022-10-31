#![no_std]

use core::{
    fmt, iter,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    slice,
};

use asr::{Address, Process};

pub use asr;
pub use asr_dotnet_derive::*;
use bytemuck::{Pod, Zeroable};

#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(transparent)]
pub struct CStr;

#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct Ptr<T = ()>(u64, PhantomData<T>);

unsafe impl<T: 'static + Copy> Pod for Ptr<T> {}
unsafe impl<T> Zeroable for Ptr<T> {}

impl<T> Ptr<T> {
    fn addr(self) -> Address {
        Address(self.0)
    }

    pub fn is_null(&self) -> bool {
        self.0 == 0
    }

    pub fn cast<U>(self) -> Ptr<U> {
        Ptr(self.0, PhantomData)
    }

    pub fn byte_offset(self, bytes: u64) -> Self {
        Self(self.0 + bytes, PhantomData)
    }

    pub fn offset(self, count: u64) -> Self {
        Self(self.0 + count * mem::size_of::<T>() as u64, PhantomData)
    }
}

impl<T: Pod> Ptr<T> {
    pub fn read(self, process: &Process) -> Result<T, ()> {
        process.read(self.addr()).map_err(drop)
    }

    fn index(self, process: &Process, idx: usize) -> Result<T, ()> {
        process
            .read(self.addr() + (idx * mem::size_of::<T>()) as u64)
            .map_err(drop)
    }
}

impl Ptr<CStr> {
    #[inline(never)]
    pub fn read_str<R>(self, process: &Process, f: impl FnOnce(&[u8]) -> R) -> R {
        let mut addr = self.addr();
        let mut buf = [MaybeUninit::<u8>::uninit(); 32 << 10];
        let mut cursor = &mut buf[..];
        let total_len = loop {
            // We round up to the 4 KiB address boundary as that's a single
            // page, which is safe to read either fully or not at all. We do
            // this to do a single read rather than many small ones as the
            // syscall overhead is a quite high.
            let end = (addr.0 & !((4 << 10) - 1)) + (4 << 10);
            // However we limit it to 256 bytes as 512 bytes is roughly the
            // break even point in terms of syscall overhead and realistic
            // string sizes are probably even smaller than that.
            let len = (end - addr.0).min(256);
            let (current_read_buf, after) = cursor.split_at_mut(len as usize);
            cursor = after;
            let current_read_buf = process
                .read_into_uninit_buf(addr, current_read_buf)
                .unwrap();
            if let Some(pos) = bstr::ByteSlice::find_byte(current_read_buf, 0) {
                let cursor_len = cursor.len();
                let current_read_buf_len = current_read_buf.len();
                let buf_len = buf.len();
                break buf_len - cursor_len - current_read_buf_len + pos;
            } else {
                addr = end.into();
            }
        };
        f(unsafe { slice::from_raw_parts(buf.as_ptr().cast(), total_len) })
    }
}

impl<T: Pod> Ptr<GList<T>> {
    pub fn iter(mut self, process: &Process) -> impl Iterator<Item = Ptr<T>> + '_ {
        iter::from_fn(move || {
            if !self.is_null() {
                let list: GList<T> = self.read(process).ok()?;
                self = list.next;
                Some(list.data)
            } else {
                None
            }
        })
    }
}

impl<T> fmt::Debug for Ptr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            f.write_str("NULL")
        } else {
            write!(f, "{:x}", self.0)
        }
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct GHashTable<K = (), V = ()> {
    hash_func: Ptr,
    key_equal_func: Ptr,

    table: Ptr<Ptr<Slot<K, V>>>,
    table_size: i32,
    in_use: i32,
    threshold: i32,
    last_rehash: i32,
    value_destroy_func: Ptr,
    key_destroy_func: Ptr,
}

unsafe impl<K: 'static + Copy, V: 'static + Copy> Pod for GHashTable<K, V> {}
unsafe impl<K, V> Zeroable for GHashTable<K, V> {}

impl<K: Pod, V: Pod> GHashTable<K, V> {
    #[cfg(not(feature = "il2cpp"))]
    fn iter<'a>(&'a self, process: &'a Process) -> impl Iterator<Item = (Ptr<K>, Ptr<V>)> + 'a {
        (0..self.table_size as usize)
            .flat_map(move |i| {
                let mut slot_ptr = self.table.index(process, i).ok()?;
                Some(core::iter::from_fn(move || {
                    if !slot_ptr.is_null() {
                        let slot: Slot<K, V> = slot_ptr.read(process).unwrap();
                        slot_ptr = slot.next;
                        Some((slot.key, slot.value))
                    } else {
                        None
                    }
                }))
            })
            .flatten()
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct Slot<K, V> {
    key: Ptr<K>,
    value: Ptr<V>,
    next: Ptr<Slot<K, V>>,
}

unsafe impl<K: 'static + Copy, V: 'static + Copy> Pod for Slot<K, V> {}
unsafe impl<K, V> Zeroable for Slot<K, V> {}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct GList<T> {
    data: Ptr<T>,
    next: Ptr<GList<T>>,
    prev: Ptr<GList<T>>,
}

unsafe impl<T: 'static + Copy> Pod for GList<T> {}
unsafe impl<T> Zeroable for GList<T> {}

#[cfg(not(feature = "il2cpp"))]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoAssembly {
    pub ref_count: i32,
    _padding: [u8; 4],
    pub basedir: Ptr<CStr>,
    pub aname: MonoAssemblyName,
    pub image: Ptr<MonoImage>,
}

#[cfg(feature = "il2cpp")]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoAssembly {
    pub image: Ptr<MonoImage>,
    pub token: u32,
    pub referenced_assembly_start: i32,
    pub referenced_assembly_count: i32,
    _padding: [u8; 4],
    pub aname: MonoAssemblyName,
}

#[cfg(not(feature = "il2cpp"))]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoAssemblyName {
    pub name: Ptr<CStr>,
    pub culture: Ptr<CStr>,
    pub hash_value: Ptr<CStr>,
    pub public_key: Ptr,
    pub public_key_token: [u8; 17],
    _padding1: [u8; 3],
    pub hash_alg: u32,
    pub hash_len: u32,
    pub flags: u32,
    pub major: MonoAssemblyNameInt,
    pub minor: MonoAssemblyNameInt,
    pub build: MonoAssemblyNameInt,
    pub revision: MonoAssemblyNameInt,
    pub arch: MonoAssemblyNameInt,
    pub without_version: MonoBoolean,
    pub without_culture: MonoBoolean,
    pub without_public_key_token: MonoBoolean,
    _padding2: [u8; 3],
}

#[cfg(feature = "il2cpp")]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoAssemblyName {
    pub name: Ptr<CStr>,
    pub culture: Ptr<CStr>,

    pub public_key: Ptr,
    pub hash_alg: u32,
    pub hash_len: i32,
    pub flags: u32,

    pub major: i32,
    pub minor: i32,
    pub build: i32,
    pub revision: i32,

    pub public_key_token: [u8; 8],
    _padding: [u8; 4],
}

#[cfg(not(feature = "il2cpp"))]
type MonoBoolean = u8;

#[cfg(not(feature = "il2cpp"))]
// u16 if netcore is not enabled
type MonoAssemblyNameInt = u16;

#[cfg(not(feature = "il2cpp"))]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoImage {
    ref_count: i32,
    _padding: [u8; 4],
    raw_data_handle: Ptr,
    raw_data: Ptr,
    raw_data_len: u32,
    various_flags: u32,
    name: Ptr<CStr>,
    assembly_name: Ptr<CStr>,
    module_name: Ptr<CStr>,
    version: Ptr<CStr>,
    md_version_major: i16,
    md_version_minor: i16,
    _padding2: [u8; 4],
    guid: Ptr<CStr>,
    image_info: Ptr,
    mempool: Ptr,
    raw_metadta: Ptr,
    heap_strings: MonoStreamHeader,
    heap_us: MonoStreamHeader,
    heap_blob: MonoStreamHeader,
    heap_guid: MonoStreamHeader,
    heap_tables: MonoStreamHeader,
    heap_pdb: MonoStreamHeader,
    tables_base: Ptr,
    referenced_tables: u64,
    referenced_table_rows: Ptr<i32>,
    tables: [MonoTableInfo; MONO_TABLE_NUM],
    references: Ptr<Ptr<MonoAssembly>>,
    nreferences: i32,
    _padding3: [u8; 4],
    modules: Ptr<Ptr<MonoImage>>,
    module_count: u32,
    _padding4: [u8; 4],
    modules_loaded: Ptr, // to gboolean
    files: Ptr<Ptr<MonoImage>>,
    file_count: u32,
    _padding5: [u8; 4],
    aot_module: Ptr,
    aotid: [u8; 16],
    assembly: Ptr<MonoAssembly>,
    method_cache: Ptr<GHashTable>,
    class_cache: MonoInternalHashTable<MonoClassDef>,
}

#[cfg(feature = "il2cpp")]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoImage {
    name: Ptr<CStr>,
    name_no_ext: Ptr<CStr>,
    assembly: Ptr<MonoAssembly>,

    type_count: u32,
    exported_type_count: u32,
    custom_attribute_count: u32,

    _padding1: [u8; 4],

    metadata_handle: Ptr<i32>,
    name_to_class_hash_table: Ptr,
    code_gen_module: Ptr,
    token: u32,
    dynamic: u8,

    _padding2: [u8; 3],
}

impl MonoImage {
    #[cfg(not(feature = "il2cpp"))]
    pub fn classes<'a>(&'a self, process: &'a Process) -> impl Iterator<Item = MonoClassDef> + 'a {
        (0..self.class_cache.size as usize).flat_map(move |i| {
            let mut class_ptr = self.class_cache.table.index(process, i).unwrap();
            iter::from_fn(move || {
                if !class_ptr.is_null() {
                    let class = class_ptr.read(process).ok()?;
                    class_ptr = class.next_class_cache.cast();
                    Some(class)
                } else {
                    None
                }
            })
        })
    }

    #[cfg(feature = "il2cpp")]
    pub fn classes<'a>(
        &'a self,
        process: &'a Process,
    ) -> Result<impl Iterator<Item = MonoClass> + 'a, ()> {
        let module = process.get_module("GameAssembly.dll").map_err(drop)?;
        let type_info_definition_table: Ptr<Ptr<MonoClass>> =
            process.read(module + 0x25CB530u64).map_err(drop)?;
        let ptr = type_info_definition_table
            .offset(self.metadata_handle.read(process).unwrap_or_default() as _);
        Ok((0..self.type_count as usize).filter_map(move |i| {
            let class_ptr = ptr.index(process, i).ok()?;
            if class_ptr.is_null() {
                None
            } else {
                class_ptr.read(process).ok()
            }
        }))
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct MonoInternalHashTable<T> {
    hash_func: Ptr,
    key_extract: Ptr,
    next_value: Ptr,
    size: i32,
    num_entries: i32,
    table: Ptr<Ptr<T>>,
}

unsafe impl<T: 'static + Copy> Pod for MonoInternalHashTable<T> {}
unsafe impl<T> Zeroable for MonoInternalHashTable<T> {}

#[cfg(not(feature = "il2cpp"))]
const MONO_TABLE_NUM: usize = 56;

#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoStreamHeader {
    data: Ptr,
    size: u32,
    _padding: [u8; 4],
}

#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoTableInfo {
    base: Ptr, // might be CStr
    rows_and_size: [u8; 3],
    row_size: u8,
    size_bitfield: u32,
}

#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoClassDef {
    pub klass: MonoClass,
    pub flags: u32,
    pub first_method_idx: u32,
    pub first_field_idx: u32,
    pub method_count: u32,
    pub field_count: u32,
    _padding: [u8; 4],
    pub next_class_cache: Ptr<MonoClass>,
}

impl MonoClassDef {
    pub fn fields<'a>(&'a self, process: &'a Process) -> impl Iterator<Item = MonoClassField> + 'a {
        (0..self.field_count as usize).flat_map(|i| self.klass.fields.index(process, i))
    }

    pub fn find_field(&self, process: &Process, name: &str) -> Option<usize> {
        Some(
            self.fields(process)
                .find(|field| field.name.read_str(process, |n| n == name.as_bytes()))?
                .offset as usize,
        )
    }

    #[cfg(not(feature = "il2cpp"))]
    pub fn find_singleton(&self, process: &Process, instance_field_name: &str) -> Result<Ptr, ()> {
        let instance_field = self.find_field(process, instance_field_name).ok_or(())?;

        let instance = self
            .klass
            .get_static_field_memory(process)?
            .byte_offset(instance_field as u64)
            .cast::<Ptr>()
            .read(process)?;

        if instance.is_null() {
            return Err(());
        }

        Ok(instance)
    }
}

#[cfg(not(feature = "il2cpp"))]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoClass {
    pub element_class: Ptr<MonoClass>,
    pub cast_class: Ptr<MonoClass>,
    pub supertypes: Ptr<Ptr<MonoClass>>,
    pub idepth: u16,
    pub rank: u8,
    _padding1: u8,
    pub instance_size: i32,
    pub flags1: u32,
    pub min_align: u8,
    _padding2: [u8; 3],
    pub flags2: u32,
    _padding3: [u8; 4],
    pub parent: Ptr<MonoClass>,
    pub nested_in: Ptr<MonoClass>,
    pub image: Ptr<MonoImage>,
    pub name: Ptr<CStr>,
    pub name_space: Ptr<CStr>,
    pub type_token: u32,
    pub vtable_size: i32,
    pub interface_count: u16,
    _padding4: [u8; 2],
    pub interface_id: u32,
    pub max_interface_id: u32,
    pub interface_offsets_count: u16,
    _padding5: [u8; 2],
    pub interfaces_packed: Ptr<Ptr<MonoClass>>,
    pub interface_offsets_packed: Ptr<u16>,
    pub interface_bitmap: Ptr<u8>,
    pub interfaces: Ptr<Ptr<MonoClass>>,
    pub sizes: i32,
    _padding6: [u8; 4],
    pub fields: Ptr<MonoClassField>,
    pub methods: Ptr<Ptr>,
    pub this_arg: MonoType,
    pub byval_arg: MonoType,
    pub gc_descr: MonoGCDescriptor,
    pub runtime_info: Ptr<MonoClassRuntimeInfo>,
    pub vtable: Ptr<Ptr>,
    pub infrequent_data: MonoPropertyBag,
    pub unity_user_data: Ptr,
}

#[cfg(feature = "il2cpp")]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoClass {
    image: Ptr<MonoImage>,
    gc_desc: Ptr,
    pub name: Ptr<CStr>,
    pub name_space: Ptr<CStr>,
    byval_arg: MonoType,
    this_arg: MonoType,

    element_class: Ptr<MonoClass>,
    cast_class: Ptr<MonoClass>,
    declaring_type: Ptr<MonoClass>,
    parent: Ptr<MonoClass>,
    generic_class: Ptr, // <MonoGenericClass>,
    type_metadata_handle: Ptr,
    interop_data: Ptr,
    klass: Ptr<MonoClass>,

    fields: Ptr<MonoClassField>,
    events: Ptr,       // <EventInfo>
    properties: Ptr,   // <PropertyInfo>
    methods: Ptr<Ptr>, // <MethodInfo>
    nested_types: Ptr<Ptr<MonoClass>>,
    implemented_interfaces: Ptr<Ptr<MonoClass>>,
    interface_offsets: Ptr,

    static_fields: Ptr,

    rgctx_data: Ptr,
    type_hierarchy: Ptr<Ptr<MonoClass>>,
    unity_user_data: Ptr,
    initialization_exception_gc_handle: u32,
    cctor_started: u32,
    cctor_finished: u32,
    _padding1: [u8; 4],
    cctor_thread: u64,
    generic_container_handle: Ptr,

    instance_size: u32,
    actual_size: u32,
    element_size: u32,
    native_size: i32,
    static_fields_size: u32,
    thread_static_fields_size: u32,
    thread_static_fields_offset: i32,

    flags: u32,
    token: u32,

    method_count: u16,
    property_count: u16,
    field_count: u16,
    event_count: u16,
    nested_type_count: u16,
    vtable_count: u16,
    interfaces_count: u16,
    interface_offsets_count: u16,

    type_hierarchy_depth: u8,
    generic_recursion_depth: u8,
    rank: u8,
    minimum_alignment: u8,
    natural_aligment: u8,
    packing_size: u8,

    more_flags: [u8; 2],
    // initialized_and_no_error: u8:1,
    // valuetype: u8:1,
    // initialized: u8:1,
    // enumtype: u8:1,
    // is_generic: u8:1,
    // has_references: u8:1,
    // init_pending: u8:1,
    // size_inited: u8:1,

    // has_finalize: u8:1,
    // has_cctor: u8:1,
    // is_blittable: u8:1,
    // is_import_or_windows_runtime: u8:1,
    // is_vtable_initialized: u8:1,
    // has_initialization_error: u8:1,
    _padding2: [u8; 4],
}

impl MonoClass {
    pub fn get_instance<R>(
        &self,
        instance: Ptr,
        process: &Process,
        f: impl FnOnce(&[u8]) -> R,
    ) -> Result<R, ()> {
        let mut buf = [0; 4 << 10];
        let buf = buf.get_mut(..self.instance_size as usize).ok_or(())?;
        process.read_into_buf(instance.addr(), buf).map_err(drop)?;
        Ok(f(buf))
    }

    #[cfg(not(feature = "il2cpp"))]
    pub fn get_static_field_memory(&self, process: &Process) -> Result<Ptr, ()> {
        self.runtime_info
            .byte_offset(mem::size_of::<MonoClassRuntimeInfo>() as u64)
            .cast::<Ptr<MonoVTable>>()
            .read(process)?
            .byte_offset(mem::size_of::<MonoVTable>() as u64)
            .cast::<Ptr<_>>()
            .index(process, self.vtable_size as usize)
    }

    #[cfg(feature = "il2cpp")]
    pub fn fields<'a>(&'a self, process: &'a Process) -> impl Iterator<Item = MonoClassField> + 'a {
        (0..self.field_count as usize).flat_map(|i| self.fields.index(process, i))
    }

    #[cfg(feature = "il2cpp")]
    pub fn find_field(&self, process: &Process, name: &str) -> Option<i32> {
        Some(
            self.fields(process)
                .find(|field| field.name.read_str(process, |n| n == name.as_bytes()))?
                .offset,
        )
    }

    #[cfg(feature = "il2cpp")]
    pub fn find_singleton(&self, process: &Process, instance_field_name: &str) -> Result<Ptr, ()> {
        let instance_field = self.find_field(process, instance_field_name).ok_or(())?;

        let instance = self
            .static_fields
            .byte_offset(instance_field as u64)
            .cast::<Ptr>()
            .read(process)?;

        if instance.is_null() {
            return Err(());
        }

        Ok(instance)
    }
}

type MonoGCDescriptor = Ptr;

#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoType {
    data: Ptr,
    attrs: u16,
    r#type: u8,
    flags: u8,
    _padding: [u8; 4],
}

#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoPropertyBag {
    head: Ptr,
}

#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoClassRuntimeInfo {
    max_domain: u16,
    _padding: [u8; 6],
}

#[cfg(not(feature = "il2cpp"))]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoClassField {
    pub r#type: Ptr<MonoType>,
    pub name: Ptr<CStr>,
    pub parent: Ptr<MonoClass>,
    pub offset: i32,
    _padding: [u8; 4],
}

#[cfg(feature = "il2cpp")]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoClassField {
    pub name: Ptr<CStr>,
    pub r#type: Ptr<MonoType>,
    pub parent: Ptr<MonoClass>,
    pub offset: i32,
    pub token: u32,
}

#[derive(Debug, Copy, Clone, Pod, Zeroable)]
#[repr(C)]
pub struct MonoVTable {
    klass: Ptr<MonoClass>,
    gc_descr: MonoGCDescriptor,
    domain: Ptr,
    r#type: Ptr,
    interface_bitmap: Ptr<u8>,
    max_interface_id: u32,
    rank: u8,
    initialized: u8,
    flags: u8,
    _padding1: u8,
    imt_collisions_bitmap: u32,
    _padding2: [u8; 4],
    runtime_generic_context: Ptr,
}

pub fn g_str_hash_with_artificial_nul_terminator(value: &[u8]) -> u32 {
    let mut hash: u32 = 0;
    value.iter().copied().chain([0]).skip(1).for_each(|c| {
        hash = (hash << 5).wrapping_sub(hash.wrapping_add(c as u32));
    });
    hash
}
