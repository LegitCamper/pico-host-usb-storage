use crate::msc::MscHandler;
use core::cell::RefCell;
use embassy_usb_driver::host::{UsbChannel, UsbHostDriver};
use embedded_sdmmc::asynchronous::{Block, BlockCount, BlockDevice, BlockIdx};
use scsi::scsi::commands::{
    Command, CommandStatusWrapper, InquiryCommand, InquiryResponse, Read10Command,
    ReadCapacityCommand, ReadCapacityResponse, TestUnitReady, Write10Command,
};
use scsi::{BufferPullable, BufferPushable};

pub const BLOCK_SIZE: usize = Block::LEN;

#[derive(Debug)]
pub enum ScsiError {
    UsbError,
}

pub struct ScsiHandler<H: UsbHostDriver> {
    msc: MscHandler<H>,
    inquiry: InquiryResponse,
    size: u32,
}

impl<H: UsbHostDriver> ScsiHandler<H> {
    pub fn new(msc_handler: MscHandler<H>) -> Self {
        // send init commands to scsi device
        Self {
            msc: msc_handler,
            inquiry: InquiryResponse::default(),
            size: 0,
        }
    }

    pub async fn init(&mut self) -> Result<(), ScsiError> {
        let _inquiry = self.inquiry().await?;
        let _ready = self.test_unit_ready().await?;
        let _size = self.read_capacity_10().await?;

        Ok(())
    }

    async fn get_csw(&mut self) -> Result<CommandStatusWrapper, ScsiError> {
        let mut csw_buf = [0u8; 13];
        self.msc
            .bulk_in
            .request_in(&mut csw_buf)
            .await
            .map_err(|_| ScsiError::UsbError)?;
        Ok(
            CommandStatusWrapper::pull_from_buffer(&csw_buf)
                .expect("Device returned malformed CSW"),
        )
    }

    async fn inquiry(&mut self) -> Result<InquiryResponse, ScsiError> {
        const INQUIRY_LEN: usize = 36;

        let mut out_buf = [0; 31];

        let cmd = InquiryCommand::new(INQUIRY_LEN as u8);
        let wrapper = cmd.wrapper();
        wrapper
            .push_to_buffer(&mut out_buf)
            .expect("CBW buffer must be at least 15 bytes");
        cmd.push_to_buffer(&mut out_buf)
            .expect("CDB buffer must be large enough for SCSI command");

        self.msc
            .bulk_out
            .request_out(&out_buf, false)
            .await
            .map_err(|_| ScsiError::UsbError)?;

        let mut inquiry = [0u8; INQUIRY_LEN];
        self.msc
            .bulk_in
            .request_in(&mut inquiry)
            .await
            .map_err(|_| ScsiError::UsbError)?;

        self.get_csw().await?;

        self.inquiry = InquiryResponse::pull_from_buffer(&inquiry)
            .expect("Device returned invalid Inquiry data");

        Ok(self.inquiry)
    }

    async fn test_unit_ready(&mut self) -> Result<CommandStatusWrapper, ScsiError> {
        let mut buf = [0u8; 31];

        let cmd = TestUnitReady::new();
        let wrapper = cmd.wrapper();
        wrapper
            .push_to_buffer(&mut buf)
            .expect("CBW buffer must be at least 15 bytes");
        cmd.push_to_buffer(&mut buf)
            .expect("CDB buffer must be large enough for SCSI command");

        self.msc
            .bulk_out
            .request_out(&buf, false)
            .await
            .map_err(|_| ScsiError::UsbError)?;

        self.get_csw().await
    }

    async fn read_capacity_10(&mut self) -> Result<ReadCapacityResponse, ScsiError> {
        let mut buf = [0u8; 31];

        let cmd = ReadCapacityCommand::new();
        let wrapper = cmd.wrapper();
        wrapper
            .push_to_buffer(&mut buf)
            .expect("CBW buffer must be at least 15 bytes");
        cmd.push_to_buffer(&mut buf)
            .expect("CDB buffer must be large enough for SCSI command");

        self.msc
            .bulk_out
            .request_out(&buf, false)
            .await
            .map_err(|_| ScsiError::UsbError)?;

        let mut data_buf = [0u8; 8];
        self.msc
            .bulk_in
            .request_in(&mut data_buf)
            .await
            .map_err(|_| ScsiError::UsbError)?;

        let capacity = ReadCapacityResponse::pull_from_buffer(&data_buf)
            .expect("Device returned invalid Capacity data");

        assert!(capacity.block_length == BLOCK_SIZE as u32);

        self.get_csw().await?;

        self.size = capacity.logical_block_address;
        Ok(capacity)
    }

    async fn read_10(
        &mut self,
        data: &mut [u8],
        block_address: u32,
        blocks: u16,
    ) -> Result<CommandStatusWrapper, ScsiError> {
        assert!(data.len() >= blocks as usize);
        let mut buf = [0u8; 31];

        let cmd = Read10Command {
            block_address,
            block_size: BLOCK_SIZE as u32,
            transfer_blocks: blocks,
        };
        let wrapper = cmd.wrapper();
        wrapper
            .push_to_buffer(&mut buf)
            .expect("CBW buffer must be at least 15 bytes");
        cmd.push_to_buffer(&mut buf)
            .expect("CDB buffer must be large enough for SCSI command");

        self.msc
            .bulk_out
            .request_out(&buf, false)
            .await
            .map_err(|_| ScsiError::UsbError)?;

        self.msc
            .bulk_in
            .request_in(data)
            .await
            .map_err(|_| ScsiError::UsbError)?;

        self.get_csw().await
    }

    async fn write_10(
        &mut self,
        data: &[u8],
        block_address: u32,
        blocks: u16,
    ) -> Result<CommandStatusWrapper, ScsiError> {
        assert!(data.len() == blocks as usize);
        let mut buf = [0u8; 31];

        let cmd = Write10Command {
            block_address,
            block_size: BLOCK_SIZE as u32,
            transfer_blocks: blocks,
        };

        let wrapper = cmd.wrapper();
        wrapper
            .push_to_buffer(&mut buf)
            .expect("CBW buffer must be at least 15 bytes");
        cmd.push_to_buffer(&mut buf)
            .expect("CDB buffer must be large enough for SCSI command");

        self.msc
            .bulk_out
            .request_out(&buf, false)
            .await
            .map_err(|_| ScsiError::UsbError)?;

        self.msc
            .bulk_out
            .request_out(data, false)
            .await
            .map_err(|_| ScsiError::UsbError)?;

        self.get_csw().await
    }
}

pub struct SdmmcScsi<H: UsbHostDriver> {
    scsi: RefCell<ScsiHandler<H>>,
}

impl<H: UsbHostDriver> SdmmcScsi<H> {
    pub fn new(scsi: ScsiHandler<H>) -> Self {
        Self { scsi: scsi.into() }
    }
}

impl<H: UsbHostDriver> BlockDevice for SdmmcScsi<H> {
    type Error = ScsiError;

    async fn read(
        &self,
        blocks: &mut [Block],
        start_block_idx: BlockIdx,
    ) -> Result<(), Self::Error> {
        let num_blocks = blocks.len();

        // SAFETY:
        // This is safe because Block is a transparent wrapper around [u8; 512]
        // and slices are guaranteed to be contiguous.
        let data_ptr = blocks.as_mut_ptr() as *mut u8;
        let data_slice =
            unsafe { core::slice::from_raw_parts_mut(data_ptr, num_blocks * BLOCK_SIZE) };

        self.scsi
            .borrow_mut()
            .read_10(data_slice, start_block_idx.0, num_blocks as u16)
            .await?;
        Ok(())
    }

    async fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        let num_blocks = blocks.len();

        // SAFETY:
        // This is safe because Block is a transparent wrapper around [u8; 512]
        // and slices are guaranteed to be contiguous.
        let data_ptr = blocks.as_ptr() as *const u8;
        let data_slice = unsafe { core::slice::from_raw_parts(data_ptr, num_blocks * BLOCK_SIZE) };

        self.scsi
            .borrow_mut()
            .write_10(data_slice, start_block_idx.0, num_blocks as u16)
            .await?;
        Ok(())
    }

    async fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        Ok(BlockCount(self.scsi.borrow().size))
    }
}
