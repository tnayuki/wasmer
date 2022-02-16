//! Relocation is the process of assigning load addresses for position-dependent
//! code and data of a program and adjusting the code and data to reflect the
//! assigned addresses.
//!
//! [Learn more](https://en.wikipedia.org/wiki/Relocation_(computing)).
//!
//! Each time a `Compiler` compiles a WebAssembly function (into machine code),
//! it also attaches if there are any relocations that need to be patched into
//! the generated machine code, so a given frontend (JIT or native) can
//! do the corresponding work to run it.

use crate::lib::std::fmt;
use crate::lib::std::vec::Vec;
use crate::section::SectionIndex;
use crate::{Addend, CodeOffset, JumpTable};
use loupe::MemoryUsage;
#[cfg(feature = "enable-rkyv")]
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
#[cfg(feature = "enable-serde")]
use serde::{Deserialize, Serialize};
use wasmer_types::entity::PrimaryMap;
use wasmer_types::LocalFunctionIndex;
use wasmer_vm::libcalls::LibCall;

/// Relocation kinds for every ISA.
#[cfg_attr(feature = "enable-serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "enable-rkyv",
    derive(RkyvSerialize, RkyvDeserialize, Archive)
)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, MemoryUsage)]
pub enum RelocationKind {
    /// absolute 4-byte
    Abs4,
    /// absolute 8-byte
    Abs8,
    /// x86 PC-relative 4-byte
    X86PCRel4,
    /// x86 PC-relative 8-byte
    X86PCRel8,
    /// x86 PC-relative 4-byte offset to trailing rodata
    X86PCRelRodata4,
    /// x86 call to PC-relative 4-byte
    X86CallPCRel4,
    /// x86 call to PLT-relative 4-byte
    X86CallPLTRel4,
    /// x86 GOT PC-relative 4-byte
    X86GOTPCRel4,
    /// Arm32 call target
    Arm32Call,
    /// Arm64 call target
    Arm64Call,
    /// Arm64 movk/z part 0
    Arm64Movw0,
    /// Arm64 movk/z part 1
    Arm64Movw1,
    /// Arm64 movk/z part 2
    Arm64Movw2,
    /// Arm64 movk/z part 3
    Arm64Movw3,
    /// RISC-V call target
    RiscvCall,
    /// Elf x86_64 32 bit signed PC relative offset to two GOT entries for GD symbol.
    ElfX86_64TlsGd,
    // /// Mach-O x86_64 32 bit signed PC relative offset to a `__thread_vars` entry.
    // MachOX86_64Tlv,
}

impl fmt::Display for RelocationKind {
    /// Display trait implementation drops the arch, since its used in contexts where the arch is
    /// already unambiguous, e.g. clif syntax with isa specified. In other contexts, use Debug.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Self::Abs4 => write!(f, "Abs4"),
            Self::Abs8 => write!(f, "Abs8"),
            Self::X86PCRel4 => write!(f, "PCRel4"),
            Self::X86PCRel8 => write!(f, "PCRel8"),
            Self::X86PCRelRodata4 => write!(f, "PCRelRodata4"),
            Self::X86CallPCRel4 => write!(f, "CallPCRel4"),
            Self::X86CallPLTRel4 => write!(f, "CallPLTRel4"),
            Self::X86GOTPCRel4 => write!(f, "GOTPCRel4"),
            Self::Arm32Call | Self::Arm64Call | Self::RiscvCall => write!(f, "Call"),
            Self::Arm64Movw0 => write!(f, "Arm64MovwG0"),
            Self::Arm64Movw1 => write!(f, "Arm64MovwG1"),
            Self::Arm64Movw2 => write!(f, "Arm64MovwG2"),
            Self::Arm64Movw3 => write!(f, "Arm64MovwG3"),
            Self::ElfX86_64TlsGd => write!(f, "ElfX86_64TlsGd"),
            // Self::MachOX86_64Tlv => write!(f, "MachOX86_64Tlv"),
        }
    }
}

/// A record of a relocation to perform.
#[cfg_attr(feature = "enable-serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "enable-rkyv",
    derive(RkyvSerialize, RkyvDeserialize, Archive)
)]
#[derive(Debug, Clone, PartialEq, Eq, MemoryUsage)]
pub struct Relocation {
    /// The relocation kind.
    pub kind: RelocationKind,
    /// Relocation target.
    pub reloc_target: RelocationTarget,
    /// The offset where to apply the relocation.
    pub offset: CodeOffset,
    /// The addend to add to the relocation value.
    pub addend: Addend,
}

/// Destination function. Can be either user function or some special one, like `memory.grow`.
#[cfg_attr(feature = "enable-serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "enable-rkyv",
    derive(RkyvSerialize, RkyvDeserialize, Archive)
)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, MemoryUsage)]
pub enum RelocationTarget {
    /// A relocation to a function defined locally in the wasm (not an imported one).
    LocalFunc(LocalFunctionIndex),
    /// A compiler-generated libcall.
    LibCall(LibCall),
    /// Jump table index.
    JumpTable(LocalFunctionIndex, JumpTable),
    /// Custom sections generated by the compiler
    CustomSection(SectionIndex),
}

impl Relocation {
    /// Given a function start address, provide the relocation relative
    /// to that address.
    ///
    /// The function returns the relocation address and the delta.
    pub fn for_address(&self, start: usize, target_func_address: u64) -> (usize, u64) {
        match self.kind {
            RelocationKind::Abs8
            | RelocationKind::Arm64Movw0
            | RelocationKind::Arm64Movw1
            | RelocationKind::Arm64Movw2
            | RelocationKind::Arm64Movw3 => {
                let reloc_address = start + self.offset as usize;
                let reloc_addend = self.addend as isize;
                let reloc_abs = target_func_address
                    .checked_add(reloc_addend as u64)
                    .unwrap();
                (reloc_address, reloc_abs)
            }
            RelocationKind::X86PCRel4 => {
                let reloc_address = start + self.offset as usize;
                let reloc_addend = self.addend as isize;
                let reloc_delta_u32 = (target_func_address as u32)
                    .wrapping_sub(reloc_address as u32)
                    .checked_add(reloc_addend as u32)
                    .unwrap();
                (reloc_address, reloc_delta_u32 as u64)
            }
            RelocationKind::X86PCRel8 => {
                let reloc_address = start + self.offset as usize;
                let reloc_addend = self.addend as isize;
                let reloc_delta = target_func_address
                    .wrapping_sub(reloc_address as u64)
                    .checked_add(reloc_addend as u64)
                    .unwrap();
                (reloc_address, reloc_delta)
            }
            RelocationKind::X86CallPCRel4 | RelocationKind::X86CallPLTRel4 => {
                let reloc_address = start + self.offset as usize;
                let reloc_addend = self.addend as isize;
                let reloc_delta_u32 = (target_func_address as u32)
                    .wrapping_sub(reloc_address as u32)
                    .wrapping_add(reloc_addend as u32);
                (reloc_address, reloc_delta_u32 as u64)
            }
            RelocationKind::Arm64Call | RelocationKind::RiscvCall => {
                let reloc_address = start + self.offset as usize;
                let reloc_addend = self.addend as isize;
                let reloc_delta_u32 = target_func_address
                    .wrapping_sub(reloc_address as u64)
                    .wrapping_add(reloc_addend as u64);
                (reloc_address, reloc_delta_u32)
            }
            // RelocationKind::X86PCRelRodata4 => {
            //     (start, target_func_address)
            // }
            _ => panic!("Relocation kind unsupported"),
        }
    }
}

/// Relocations to apply to function bodies.
pub type Relocations = PrimaryMap<LocalFunctionIndex, Vec<Relocation>>;
