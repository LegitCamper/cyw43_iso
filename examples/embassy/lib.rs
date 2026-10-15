//! Compile-checked connection to the real Embassy CYW43 transport.
//!
//! Application must run CYW43's runner, negotiate an ISO connection and HCI data
//! path, supply an allocator for SDU/credit state, and route inbound HCI events.
//! Stock CYW43439 cannot negotiate those ISO connections.
#![no_std]

#[cfg(feature = "runtime-installer")]
pub mod runtime;

#[cfg(feature = "transport")]
use cyw43::bluetooth::{BtDriver, Error as TransportError};
#[cfg(feature = "transport")]
use cyw43_iso_re::{
    embassy::{Error, IsoTx},
    iso::{Direction, IsoPacket, PacketStatus, SduHeader},
    iso_sdu::{SduError, SduFragments},
};

/// Prepared SDU iterator; retain it across packet-credit waits.
#[cfg(feature = "transport")]
pub struct OutgoingSdu<'a> {
    fragments: SduFragments<'a>,
    pending: Option<IsoPacket<'a>>,
}

#[cfg(feature = "transport")]
impl<'a> OutgoingSdu<'a> {
    /// Prepare an LC3 SDU after codec/QoS and CIS/BIS setup have succeeded.
    ///
    /// # Errors
    /// Rejects oversized SDUs or invalid controller limits/handles.
    pub fn new(
        tx: &IsoTx,
        handle: u16,
        sequence: u16,
        timestamp: Option<u32>,
        lc3_sdu: &'a [u8],
    ) -> Result<Self, SduError> {
        Ok(Self {
            fragments: SduFragments::new(
                handle,
                SduHeader {
                    timestamp,
                    sequence_number: sequence,
                    length: u16::try_from(lc3_sdu.len()).map_err(|_| SduError::Length)?,
                    status: PacketStatus::Valid,
                },
                lc3_sdu,
                usize::from(tx.packet_length()),
                Direction::HostToController,
            )?,
            pending: None,
        })
    }

    /// Submit one fragment; `false` means all fragments have been queued.
    /// On exhausted credits, service HCI events and retry with the same iterator.
    ///
    /// # Errors
    /// Returns adapter/transport errors. Failed or cancelled transport writes
    /// require controller reset and discarding this SDU and transmit state.
    pub async fn send_next(
        &mut self,
        driver: &BtDriver<'_>,
        tx: &mut IsoTx,
    ) -> Result<bool, Error<TransportError>> {
        if self.pending.is_none() {
            self.pending = self.fragments.next();
        }
        let Some(packet) = &self.pending else {
            return Ok(false);
        };
        tx.send(driver, packet).await?;
        self.pending = None;
        Ok(true)
    }
}
