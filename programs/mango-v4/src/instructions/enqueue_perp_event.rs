use crate::accounts_ix::*;
use crate::error::*;
use crate::error_msg;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    ed25519_program, instruction::Instruction, program_error::ProgramError, sysvar::instructions,
};
use bytemuck::{Pod, Zeroable};
use ed25519_dalek::{PublicKey as DalekPublicKey, Signature as DalekSignature, Verifier};

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

pub fn enqueue_perp_event(
    ctx: Context<EnqueuePerpEvent>,
    args: EnqueuePerpEventArgs,
) -> Result<()> {
    require!(
        args.event_type.to_perp_action().is_some(),
        MangoError::UnsupportedQueueEventType
    );

    let group = ctx.accounts.group.load()?;
    let continuum = group
        .continuum_key()
        .ok_or(MangoError::ContinuumKeyMissing)?;

    let mut queue = ctx.accounts.queue.load_mut()?;
    if queue.is_full() {
        msg!(
            "enqueue_perp_event: rejected seq_no={} reason=queue_full",
            args.seq_no
        );
        return err!(MangoError::QueueIsFull);
    }

    let tail_seq = queue.tail_seq();
    if args.seq_no <= tail_seq {
        msg!(
            "enqueue_perp_event: rejected seq_no={} reason=non_monotonic tail_seq={}",
            args.seq_no,
            tail_seq
        );
        return err!(MangoError::SequenceNumberTooLow);
    }

    let submitted_slot = Clock::get()?.slot;
    let params: CompactOrderParams = args.params.into();
    let mut event = QueueFifoEvent {
        seq_no: args.seq_no,
        submitted_slot,
        user: args.user,
        continuum,
        event_type: args.event_type as u8,
        padding: [0u8; 7],
        params,
        user_signature: SignatureBlob {
            bytes: args.user_signature,
            is_present: 1,
            padding: [0u8; 7],
        },
        continuum_signature: SignatureBlob {
            bytes: args.continuum_signature,
            is_present: 1,
            padding: [0u8; 7],
        },
    };

    let Some(intent_signature) = extract_intent_signature(&event, &ctx.accounts.instructions)? else {
        msg!(
            "enqueue_perp_event: rejected seq_no={} reason=missing_intent_signature",
            args.seq_no
        );
        return err!(MangoError::MissingIntentSignature);
    };

    event.user_signature.bytes = intent_signature;

    verify_continuum_signature(&event)?;

    let index = queue.next_index();
    queue.entries[index] = event;
    queue.header.count = queue.header.count.saturating_add(1);

    msg!(
        "enqueue_perp_event: accepted seq_no={} queue_count={} tail_seq={}",
        args.seq_no,
        queue.header.count,
        tail_seq
    );

    Ok(())
}

fn extract_intent_signature(
    event: &QueueFifoEvent,
    instructions_ai: &AccountInfo,
) -> Result<Option<[u8; SIGNATURE_SERIALIZED_SIZE]>> {
    let expected_message = event.intent_hash();
    let mut index = 0usize;
    loop {
        match instructions::load_instruction_at_checked(index, instructions_ai) {
            Ok(ix) => {
                if let Some(signature) =
                    ed25519_instruction_matches(&ix, &event.user, &expected_message)?
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

fn verify_continuum_signature(event: &QueueFifoEvent) -> Result<()> {
    let public_key = DalekPublicKey::from_bytes(event.continuum.as_ref())
        .map_err(|_| error!(MangoError::InvalidContinuumSignature))?;
    let signature = DalekSignature::from_bytes(&event.continuum_signature.bytes)
        .map_err(|_| error!(MangoError::InvalidContinuumSignature))?;
    public_key
        .verify(&event.payload_hash(), &signature)
        .map_err(|_| error!(MangoError::InvalidContinuumSignature))
}
