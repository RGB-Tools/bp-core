// Deterministic bitcoin commitments library, implementing LNPBP standards
// Part of bitcoin protocol core library (BP Core Lib)
//
// Written in 2020-2022 by
//     Dr. Maxim Orlovsky <orlovsky@pandoracore.com>
//
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the Apache 2.0 License
// along with this software.
// If not, see <https://opensource.org/licenses/Apache-2.0>.

//! Anchors are data structures used in deterministic bitcoin commitments for
//! keeping information about the proof of the commitment in connection to the
//! transaction which contains the commitment, and multi-protocol merkle tree as
//! defined by LNPBP-4.

use std::cmp::Ordering;
use std::io::Write;

use amplify::Wrapper;
use bitcoin::hashes::{sha256, sha256t};
use bitcoin::{Script, Transaction, Txid};
use commit_verify::convolve_commit::ConvolveCommitProof;
use commit_verify::lnpbp4::{self, Message, ProtocolId};
use commit_verify::{
    CommitEncode, CommitVerify, ConsensusCommit, PrehashedProtocol, TaggedHash,
};
#[cfg(feature = "wallet")]
use commit_verify::{
    EmbedCommitProof, EmbedCommitProofStatic, EmbedCommitVerify,
    EmbedCommitVerifyStatic, TryCommitVerify, TryCommitVerifyStatic,
};
#[cfg(feature = "wallet")]
use psbt::Psbt;
use strict_encoding::StrictEncode;

#[cfg(feature = "wallet")]
use crate::tapret::{Lnpbp6, PsbtCommitError, PsbtVerifyError};
use crate::tapret::{TapretError, TapretProof};

/// Default depth of LNPBP-4 commitment tree
pub const ANCHOR_MIN_LNPBP4_DEPTH: u8 = 3;

static MIDSTATE_ANCHOR_ID: [u8; 32] = [
    148, 72, 59, 59, 150, 173, 163, 140, 159, 237, 69, 118, 104, 132, 194, 110,
    250, 108, 1, 140, 74, 248, 152, 205, 70, 32, 184, 87, 20, 102, 127, 20,
];

/// Tag used for [`AnchorId`] hash type
pub struct AnchorIdTag;

impl sha256t::Tag for AnchorIdTag {
    #[inline]
    fn engine() -> sha256::HashEngine {
        let midstate = sha256::Midstate::from_inner(MIDSTATE_ANCHOR_ID);
        sha256::HashEngine::from_midstate(midstate, 64)
    }
}

/// Unique anchor identifier equivalent to the anchor commitment hash
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", transparent)
)]
#[derive(
    Wrapper, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, From
)]
#[wrapper(
    Debug, Display, LowerHex, Index, IndexRange, IndexFrom, IndexTo, IndexFull
)]
pub struct AnchorId(sha256t::Hash<AnchorIdTag>);

impl<Msg> CommitVerify<Msg, PrehashedProtocol> for AnchorId
where
    Msg: AsRef<[u8]>,
{
    #[inline]
    fn commit(msg: &Msg) -> AnchorId { AnchorId::hash(msg) }
}

impl strict_encoding::Strategy for AnchorId {
    type Strategy = strict_encoding::strategies::Wrapped;
}

#[cfg(feature = "wallet")]
/// Errors working with anchors.
#[derive(Clone, Eq, PartialEq, Hash, Debug, Display, Error, From)]
#[display(inner)]
pub enum Error {
    /// Errors embedding LNPBP-4 commitment into PSBT
    #[from]
    EmbedCommit(PsbtCommitError),

    /// Errors constructing LNPBP-4 commitment
    #[from]
    Lnpbp4(lnpbp4::Error),
}

/// Errors verifying anchors.
#[derive(Clone, Eq, PartialEq, Debug, Display, Error, From)]
#[display(inner)]
pub enum VerifyError {
    /// Tapret commitment verification failure.
    #[from]
    Tapret(TapretError),

    /// LNPBP-4 invalid proof.
    #[from(lnpbp4::UnrelatedProof)]
    Lnpbp4UnrelatedProtocol,
}

/// Anchor is a data structure used in deterministic bitcoin commitments for
/// keeping information about the proof of the commitment in connection to the
/// transaction which contains the commitment, and multi-protocol merkle tree as
/// defined by LNPBP-4.
#[derive(Clone, PartialEq, Eq, Debug, StrictEncode, StrictDecode)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate")
)]
pub struct Anchor<L: lnpbp4::Proof> {
    /// Transaction containing deterministic bitcoin commitment.
    pub txid: Txid,

    /// Structured multi-protocol LNPBP-4 data the transaction commits to.
    pub lnpbp4_proof: L,

    /// Proof of the DBC commitment.
    pub dbc_proof: Proof,
}

impl CommitEncode for Anchor<lnpbp4::MerkleBlock> {
    fn commit_encode<E: Write>(&self, mut e: E) -> usize {
        let mut len = self
            .txid
            .strict_encode(&mut e)
            .expect("memory encoders do not fail");
        len += self
            .dbc_proof
            .strict_encode(&mut e)
            .expect("memory encoders do not fail");
        len + self.lnpbp4_proof.commit_encode(e)
    }
}

impl ConsensusCommit for Anchor<lnpbp4::MerkleBlock> {
    type Commitment = AnchorId;
}

impl Ord for Anchor<lnpbp4::MerkleBlock> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.anchor_id().cmp(&other.anchor_id())
    }
}

impl PartialOrd for Anchor<lnpbp4::MerkleBlock> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Error merging two [`Anchor`]s.
#[derive(
    Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug, Display, Error,
    From
)]
#[display(doc_comments)]
pub enum MergeError {
    /// Error merging two LNPBP-4 proofs, which are unrelated.
    #[display(inner)]
    #[from(lnpbp4::UnrelatedProof)]
    Lnpbp4Mismatch,

    /// anchors can't be merged since they have different witness transactions
    TxidMismatch,

    /// anchors can't be merged since they have different proofs
    ProofMismatch,
}

impl Anchor<lnpbp4::MerkleBlock> {
    /// Returns id of the anchor (commitment hash).
    #[inline]
    pub fn anchor_id(&self) -> AnchorId { self.consensus_commit() }

    /// Convenience constructor for anchor, which also does embedding of LNPBP4
    /// commitment into PSBT.
    #[cfg(feature = "wallet")]
    pub fn commit(
        psbt: &mut Psbt,
    ) -> Result<Anchor<lnpbp4::MerkleBlock>, Error> {
        let anchor = psbt.embed_commit(&PsbtEmbeddedMessage)?;
        Ok(Anchor {
            txid: anchor.txid,
            lnpbp4_proof: lnpbp4::MerkleBlock::from(anchor.lnpbp4_proof),
            dbc_proof: anchor.dbc_proof,
        })
    }

    /// Static entropy version of the commit method
    #[cfg(feature = "wallet")]
    pub fn commit_static(
        psbt: &mut Psbt,
    ) -> Result<Anchor<lnpbp4::MerkleBlock>, Error> {
        let anchor = psbt.embed_commit_static(&PsbtEmbeddedMessage)?;
        Ok(Anchor {
            txid: anchor.txid,
            lnpbp4_proof: lnpbp4::MerkleBlock::from(anchor.lnpbp4_proof),
            dbc_proof: anchor.dbc_proof,
        })
    }
}

impl Anchor<lnpbp4::MerkleProof> {
    /// Returns id of the anchor (commitment hash).
    #[inline]
    pub fn anchor_id(
        &self,
        protocol_id: impl Into<ProtocolId>,
        message: Message,
    ) -> Result<AnchorId, lnpbp4::UnrelatedProof> {
        Ok(self.to_merkle_block(protocol_id, message)?.anchor_id())
    }

    /// Reconstructs anchor containing merkle block
    pub fn into_merkle_block(
        self,
        protocol_id: impl Into<ProtocolId>,
        message: Message,
    ) -> Result<Anchor<lnpbp4::MerkleBlock>, lnpbp4::UnrelatedProof> {
        let lnpbp4_proof = lnpbp4::MerkleBlock::with(
            &self.lnpbp4_proof,
            protocol_id.into(),
            message,
        )?;
        Ok(Anchor {
            txid: self.txid,
            lnpbp4_proof,
            dbc_proof: self.dbc_proof,
        })
    }

    /// Reconstructs anchor containing merkle block
    pub fn to_merkle_block(
        &self,
        protocol_id: impl Into<ProtocolId>,
        message: Message,
    ) -> Result<Anchor<lnpbp4::MerkleBlock>, lnpbp4::UnrelatedProof> {
        self.clone().into_merkle_block(protocol_id, message)
    }

    /// Verifies that the transaction commits to the anchor and the anchor
    /// commits to the given message under the given protocol.
    pub fn verify(
        &self,
        protocol_id: impl Into<ProtocolId>,
        message: Message,
        tx: Transaction,
    ) -> Result<bool, VerifyError> {
        self.dbc_proof
            .verify(
                &self.lnpbp4_proof.convolve(protocol_id.into(), message)?,
                tx,
            )
            .map_err(VerifyError::from)
    }

    /// Verifies that the anchor commits to the given message under the given
    /// protocol.
    pub fn convolve(
        &self,
        protocol_id: impl Into<ProtocolId>,
        message: Message,
    ) -> Result<lnpbp4::CommitmentHash, lnpbp4::UnrelatedProof> {
        self.lnpbp4_proof.convolve(protocol_id.into(), message)
    }
}

impl Anchor<lnpbp4::MerkleBlock> {
    /// Conceals all LNPBP-4 data except specific protocol and produces merkle
    /// proof anchor.
    pub fn to_merkle_proof(
        &self,
        protocol: impl Into<ProtocolId>,
    ) -> Result<Anchor<lnpbp4::MerkleProof>, lnpbp4::LeafNotKnown> {
        self.clone().into_merkle_proof(protocol)
    }

    /// Conceals all LNPBP-4 data except specific protocol and converts anchor
    /// into merkle proof anchor.
    pub fn into_merkle_proof(
        self,
        protocol: impl Into<ProtocolId>,
    ) -> Result<Anchor<lnpbp4::MerkleProof>, lnpbp4::LeafNotKnown> {
        let lnpbp4_proof =
            self.lnpbp4_proof.to_merkle_proof(protocol.into())?;
        Ok(Anchor {
            txid: self.txid,
            lnpbp4_proof,
            dbc_proof: self.dbc_proof,
        })
    }

    /// Conceals all LNPBP-4 data except specific protocol.
    pub fn conceal_except(
        &mut self,
        protocols: impl AsRef<[ProtocolId]>,
    ) -> Result<usize, lnpbp4::LeafNotKnown> {
        self.lnpbp4_proof.conceal_except(protocols)
    }

    /// Merges two anchors keeping revealed data.
    pub fn merge_reveal(mut self, other: Self) -> Result<Self, MergeError> {
        if self.txid != other.txid {
            return Err(MergeError::TxidMismatch);
        }
        if self.dbc_proof != other.dbc_proof {
            return Err(MergeError::ProofMismatch);
        }
        self.lnpbp4_proof.merge_reveal(other.lnpbp4_proof)?;
        Ok(self)
    }
}

/// Empty type indicating that the message has to be taken from PSBT proprietary
/// keys
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct PsbtEmbeddedMessage;

impl CommitEncode for PsbtEmbeddedMessage {
    fn commit_encode<E: Write>(&self, _: E) -> usize { 0 }
}

#[cfg(feature = "wallet")]
impl EmbedCommitProof<PsbtEmbeddedMessage, Psbt, Lnpbp6>
    for Anchor<lnpbp4::MerkleTree>
{
    fn restore_original_container(
        &self,
        psbt: &Psbt,
    ) -> Result<Psbt, PsbtVerifyError> {
        match self.dbc_proof {
            Proof::OpretFirst => Ok(psbt.clone()),
            Proof::TapretFirst(ref proof) => {
                let mut psbt = psbt.clone();
                for output in &mut psbt.outputs {
                    if output.is_tapret_host() {
                        *output = EmbedCommitProof::<_, psbt::Output, Lnpbp6>::restore_original_container(proof, output)?;
                        return Ok(psbt);
                    }
                }
                Err(PsbtVerifyError::Commit(
                    PsbtCommitError::CommitmentImpossible,
                ))
            }
        }
    }
}

#[cfg(feature = "wallet")]
impl EmbedCommitProofStatic<PsbtEmbeddedMessage, Psbt, Lnpbp6>
    for Anchor<lnpbp4::MerkleTree>
{
    fn restore_original_container(
        &self,
        psbt: &Psbt,
    ) -> Result<Psbt, PsbtVerifyError> {
        match self.dbc_proof {
            Proof::OpretFirst => Ok(psbt.clone()),
            Proof::TapretFirst(ref proof) => {
                let mut psbt = psbt.clone();
                for output in &mut psbt.outputs {
                    if output.is_tapret_host() {
                        *output = EmbedCommitProof::<_, psbt::Output, Lnpbp6>::restore_original_container(proof, output)?;
                        return Ok(psbt);
                    }
                }
                Err(PsbtVerifyError::Commit(
                    PsbtCommitError::CommitmentImpossible,
                ))
            }
        }
    }
}

#[cfg(feature = "wallet")]
impl EmbedCommitVerify<PsbtEmbeddedMessage, Lnpbp6> for Psbt {
    type Proof = Anchor<lnpbp4::MerkleTree>;
    type CommitError = PsbtCommitError;
    type VerifyError = PsbtVerifyError;

    fn embed_commit(
        &mut self,
        _: &PsbtEmbeddedMessage,
    ) -> Result<Self::Proof, Self::CommitError> {
        let lnpbp4_tree =
            |output: &mut psbt::Output| -> Result<_, PsbtCommitError> {
                let messages = output.lnpbp4_message_map()?;
                let min_depth = output
                    .lnpbp4_min_tree_depth()?
                    .unwrap_or(ANCHOR_MIN_LNPBP4_DEPTH);
                let multi_source = lnpbp4::MultiSource {
                    min_depth,
                    messages,
                };
                Ok(lnpbp4::MerkleTree::try_commit(&multi_source)?)
            };

        let (dbc_proof, lnpbp4_proof) = if let Some(output) =
            self.outputs.iter_mut().find(|o| o.is_tapret_host())
        {
            let tree = lnpbp4_tree(output)?;
            let commitment = tree.consensus_commit();
            let proof = output.embed_commit(&commitment)?;
            output.set_tapret_commitment(commitment.into_array(), &proof)?;
            output.set_lnpbp4_entropy(tree.entropy())?;
            (Proof::TapretFirst(proof), tree)
        } else if let Some(output) =
            self.outputs.iter_mut().find(|o| o.is_opret_host())
        {
            let tree = lnpbp4_tree(output)?;
            let commitment = tree.consensus_commit();
            output.script = Script::new_op_return(commitment.as_slice()).into();
            output.set_opret_commitment(commitment.into_array())?;
            output.set_lnpbp4_entropy(tree.entropy())?;
            (Proof::OpretFirst, tree)
        } else {
            return Err(PsbtCommitError::CommitmentImpossible);
        };

        Ok(Anchor {
            txid: self.to_txid(),
            lnpbp4_proof,
            dbc_proof,
        })
    }
}

#[cfg(feature = "wallet")]
impl EmbedCommitVerifyStatic<PsbtEmbeddedMessage, Lnpbp6> for Psbt {
    type Proof = Anchor<lnpbp4::MerkleTree>;
    type CommitError = PsbtCommitError;
    type VerifyError = PsbtVerifyError;

    fn embed_commit_static(
        &mut self,
        _: &PsbtEmbeddedMessage,
    ) -> Result<Self::Proof, Self::CommitError> {
        let lnpbp4_tree =
            |output: &mut psbt::Output| -> Result<_, PsbtCommitError> {
                let messages = output.lnpbp4_message_map()?;
                let min_depth = output
                    .lnpbp4_min_tree_depth()?
                    .unwrap_or(ANCHOR_MIN_LNPBP4_DEPTH);
                let multi_source = lnpbp4::MultiSource {
                    min_depth,
                    messages,
                };
                Ok(lnpbp4::MerkleTree::try_commit_static(&multi_source)?)
            };

        let (dbc_proof, lnpbp4_proof) = if let Some(output) =
            self.outputs.iter_mut().find(|o| o.is_tapret_host())
        {
            let tree = lnpbp4_tree(output)?;
            let commitment = tree.consensus_commit();
            let proof = output.embed_commit(&commitment)?;
            output.set_tapret_commitment(commitment.into_array(), &proof)?;
            output.set_lnpbp4_entropy(1)?;
            (Proof::TapretFirst(proof), tree)
        } else if let Some(output) =
            self.outputs.iter_mut().find(|o| o.is_opret_host())
        {
            let tree = lnpbp4_tree(output)?;
            let commitment = tree.consensus_commit();
            output.script = Script::new_op_return(commitment.as_slice()).into();
            output.set_opret_commitment(commitment.into_array())?;
            output.set_lnpbp4_entropy(1)?;
            (Proof::OpretFirst, tree)
        } else {
            return Err(PsbtCommitError::CommitmentImpossible);
        };

        Ok(Anchor {
            txid: self.to_txid(),
            lnpbp4_proof,
            dbc_proof,
        })
    }
}

/// Type and type-specific proof information of a deterministic bitcoin
/// commitment.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate")
)]
#[derive(StrictEncode, StrictDecode)]
#[strict_encoding(by_order)]
#[non_exhaustive]
pub enum Proof {
    /// Opret commitment (no extra-transaction proof is required).
    OpretFirst,

    /// Tapret commitment and a proof of it.
    TapretFirst(TapretProof),
}

impl Proof {
    /// Verifies validity of the proof.
    pub fn verify(
        &self,
        msg: &lnpbp4::CommitmentHash,
        tx: Transaction,
    ) -> Result<bool, TapretError> {
        match self {
            Proof::OpretFirst => {
                for txout in &tx.output {
                    if txout.script_pubkey.is_op_return() {
                        return Ok(txout.script_pubkey
                            == Script::new_op_return(msg.as_slice()));
                    }
                }
                Ok(false)
            }
            Proof::TapretFirst(proof) => {
                ConvolveCommitProof::<_, Transaction, _>::verify(proof, msg, tx)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use commit_verify::tagged_hash;

    use super::*;

    #[test]
    fn test_anchor_id_midstate() {
        let midstate = tagged_hash::Midstate::with(b"bp:dbc:anchor");
        assert_eq!(midstate.into_inner().into_inner(), MIDSTATE_ANCHOR_ID);
    }
}
