use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    ed25519_program, instruction::Instruction, program_error::ProgramError, sysvar::instructions,
};
use bytemuck::{Pod, Zeroable};

use crate::error_msg;

const SIGNATURE_OFFSETS_SERIALIZED_SIZE: usize = 14;
const SIGNATURE_OFFSETS_START: usize = 2;
const SIGNATURE_SERIALIZED_SIZE: usize = 64;
const PUBKEY_SERIALIZED_SIZE: usize = 32;

#[repr(C)]
#[derive(Copy, Clone, Default, Pod, Zeroable)]
pub struct Ed25519SignatureOffsets {
    signature_offset: u16,
    signature_instruction_index: u16,
    public_key_offset: u16,
    public_key_instruction_index: u16,
    message_data_offset: u16,
    message_data_size: u16,
    message_instruction_index: u16,
}

pub fn find_ed25519_signature(
    instructions_ai: &AccountInfo,
    expected_pubkey: &Pubkey,
    expected_message: &[u8],
) -> Result<Option<[u8; SIGNATURE_SERIALIZED_SIZE]>> {
    let mut index = 0usize;
    loop {
        match instructions::load_instruction_at_checked(index, instructions_ai) {
            Ok(ix) => {
                if let Some(signature) =
                    ed25519_instruction_matches(&ix, expected_pubkey, expected_message)?
                {
                    return Ok(Some(signature));
                }
                index += 1;
            }
            Err(ProgramError::InvalidArgument) => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn find_two_ed25519_signatures(
    instructions_ai: &AccountInfo,
    first_pubkey: &Pubkey,
    first_message: &[u8],
    second_pubkey: &Pubkey,
    second_message: &[u8],
) -> Result<(
    Option<[u8; SIGNATURE_SERIALIZED_SIZE]>,
    Option<[u8; SIGNATURE_SERIALIZED_SIZE]>,
)> {
    let mut index = 0usize;
    let mut first_signature = None;
    let mut second_signature = None;

    loop {
        match instructions::load_instruction_at_checked(index, instructions_ai) {
            Ok(ix) => {
                if first_signature.is_none() {
                    if let Some(signature) =
                        ed25519_instruction_matches(&ix, first_pubkey, first_message)?
                    {
                        first_signature = Some(signature);
                    }
                }

                if second_signature.is_none() {
                    if let Some(signature) =
                        ed25519_instruction_matches(&ix, second_pubkey, second_message)?
                    {
                        second_signature = Some(signature);
                    }
                }

                if first_signature.is_some() && second_signature.is_some() {
                    return Ok((first_signature, second_signature));
                }

                index += 1;
            }
            Err(ProgramError::InvalidArgument) => return Ok((first_signature, second_signature)),
            Err(error) => return Err(error.into()),
        }
    }
}

fn ed25519_instruction_matches(
    ix: &Instruction,
    expected_pubkey: &Pubkey,
    expected_message: &[u8],
) -> Result<Option<[u8; SIGNATURE_SERIALIZED_SIZE]>> {
    if ix.program_id != ed25519_program::id() || ix.data.len() < SIGNATURE_OFFSETS_START {
        return Ok(None);
    }
    let num_signatures = ix.data[0] as usize;
    let required_prefix = SIGNATURE_OFFSETS_START
        .saturating_add(num_signatures.saturating_mul(SIGNATURE_OFFSETS_SERIALIZED_SIZE));
    if num_signatures == 0 || ix.data.len() < required_prefix {
        return Ok(None);
    }

    for sig_index in 0..num_signatures {
        let start = SIGNATURE_OFFSETS_START + sig_index * SIGNATURE_OFFSETS_SERIALIZED_SIZE;
        let end = start + SIGNATURE_OFFSETS_SERIALIZED_SIZE;
        let offsets: &Ed25519SignatureOffsets = bytemuck::try_from_bytes(&ix.data[start..end])
            .map_err(|_| error_msg!("ed25519: invalid offset layout"))?;

        if offsets.signature_instruction_index != u16::MAX
            || offsets.public_key_instruction_index != u16::MAX
            || offsets.message_instruction_index != u16::MAX
        {
            continue;
        }

        let pk_offset = offsets.public_key_offset as usize;
        let msg_offset = offsets.message_data_offset as usize;
        let msg_size = offsets.message_data_size as usize;

        let signature_offset = offsets.signature_offset as usize;

        if pk_offset.saturating_add(PUBKEY_SERIALIZED_SIZE) > ix.data.len()
            || signature_offset.saturating_add(SIGNATURE_SERIALIZED_SIZE) > ix.data.len()
            || msg_offset.saturating_add(msg_size) > ix.data.len()
        {
            continue;
        }

        if &ix.data[pk_offset..pk_offset + PUBKEY_SERIALIZED_SIZE] != expected_pubkey.as_ref() {
            continue;
        }

        if msg_size != expected_message.len() {
            continue;
        }

        if &ix.data[msg_offset..msg_offset + msg_size] != expected_message {
            continue;
        }

        let mut signature = [0u8; SIGNATURE_SERIALIZED_SIZE];
        signature.copy_from_slice(
            &ix.data[signature_offset..signature_offset + SIGNATURE_SERIALIZED_SIZE],
        );

        return Ok(Some(signature));
    }

    Ok(None)
}
