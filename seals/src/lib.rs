// BP Core Library implementing LNP/BP specifications & standards related to
// bitcoin protocol
//
// Written in 2020-2021 by
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

#[macro_use]
extern crate amplify;
#[macro_use]
extern crate strict_encoding;

mod blind;
mod error;
mod txout_seal;
mod txout_witness;

pub use blind::{OutpointHash, OutpointReveal, ParseError};
pub use error::Error;
pub use txout_seal::{TxResolve, TxoutSeal};
pub use txout_witness::{InnerWitness, OuterWitness, Witness};