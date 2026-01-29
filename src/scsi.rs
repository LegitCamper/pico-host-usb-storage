use core::cell::RefCell;

use crate::msc::MscHandler;
use defmt::info;
use embassy_usb_driver::host::{UsbChannel, UsbHostDriver};
use embedded_sdmmc::asynchronous::{Block, BlockCount, BlockDevice, BlockIdx};
use scsi::scsi::commands::{
    Command, CommandStatusWrapper, InquiryCommand, InquiryResponse, Read10Command,
    ReadCapacityCommand, ReadCapacityResponse, TestUnitReady, Write10Command,
};
use scsi::{BufferPullable, BufferPushable};

pub struct ScsiHandler<H: UsbHostDriver> {
    msc: MscHandler<H>,
    inquiry: InquiryResponse,
    size: u32,
}

const BULK_ENDPOINT_PACKET_SIZE: usize = 64;
const BLOCK_SIZE: usize = Block::LEN;

impl<H: UsbHostDriver> ScsiHandler<H> {
    pub fn new(msc_handler: MscHandler<H>) -> Self {
        // send init commands to scsi device
        Self {
            msc: msc_handler,
            inquiry: InquiryResponse::default(),
            size: 0,
        }
    }

    pub async fn init(&mut self) {
        let _inquiry = self.inquiry().await;

        let _ready = self.test_unit_ready().await;

        let size = self.read_capacity_10().await;

        info!("block len: {:?}", size.block_length);
        info!("num blocks: {:?}", size.logical_block_address);
    }

    async fn inquiry(&mut self) -> InquiryResponse {
        const INQUIRY_LEN: usize = 36;

        let mut out_buf = [0; 31];

        let cmd = InquiryCommand::new(INQUIRY_LEN as u8);
        let wrapper = cmd.wrapper();
        wrapper.push_to_buffer(&mut out_buf).unwrap();
        cmd.push_to_buffer(&mut out_buf).unwrap();

        self.msc.bulk_out.request_out(&out_buf, true).await.unwrap();

        let mut inquiry = [0u8; INQUIRY_LEN];
        self.msc.bulk_in.request_in(&mut inquiry).await.unwrap();

        let mut csw = [0u8; 13];
        self.msc.bulk_in.request_in(&mut csw).await.unwrap();

        self.inquiry = InquiryResponse::pull_from_buffer(&inquiry).unwrap();
        self.inquiry
    }

    async fn test_unit_ready(&mut self) -> CommandStatusWrapper {
        let mut buf = [0u8; 31];

        let cmd = TestUnitReady::new();
        let wrapper = cmd.wrapper();
        wrapper.push_to_buffer(&mut buf).unwrap();
        cmd.push_to_buffer(&mut buf).unwrap();

        self.msc.bulk_out.request_out(&buf, true).await.unwrap();

        let mut csw_buf = [0u8; 13];
        self.msc.bulk_in.request_in(&mut csw_buf).await.unwrap();

        CommandStatusWrapper::pull_from_buffer(&csw_buf).unwrap()
    }

    async fn read_capacity_10(&mut self) -> ReadCapacityResponse {
        let mut buf = [0u8; 31];

        let cmd = ReadCapacityCommand::new();
        let wrapper = cmd.wrapper();
        wrapper.push_to_buffer(&mut buf).unwrap();
        cmd.push_to_buffer(&mut buf).unwrap();

        self.msc.bulk_out.request_out(&buf, true).await.unwrap();

        let mut csw_buf = [0u8; 13];
        self.msc.bulk_in.request_in(&mut csw_buf).await.unwrap();

        let capacity = ReadCapacityResponse::pull_from_buffer(&csw_buf).unwrap();

        assert!(capacity.block_length == BLOCK_SIZE as u32);

        self.size = capacity.logical_block_address;
        capacity
    }

    async fn read_10(
        &mut self,
        data: &mut [u8],
        block_address: u32,
        transfer_blocks: u16,
    ) -> CommandStatusWrapper {
        let mut buf = [0u8; 31];

        let cmd =
            Read10Command::new(block_address, BLOCK_SIZE as u32, transfer_blocks.into()).unwrap();
        let wrapper = cmd.wrapper();
        wrapper.push_to_buffer(&mut buf).unwrap();
        cmd.push_to_buffer(&mut buf).unwrap();

        self.msc.bulk_out.request_out(&buf, true).await.unwrap();

        let total_bytes = BLOCK_SIZE * transfer_blocks as usize;
        let mut offset = 0;
        while offset < total_bytes {
            let chunk_size = core::cmp::min(total_bytes - offset, BULK_ENDPOINT_PACKET_SIZE);
            self.msc
                .bulk_in
                .request_in(&mut data[offset..offset + chunk_size])
                .await
                .unwrap();
            offset += chunk_size;
        }

        let mut csw_buf = [0u8; 13];
        self.msc.bulk_in.request_in(&mut csw_buf).await.unwrap();
        CommandStatusWrapper::pull_from_buffer(&csw_buf).unwrap()
    }

    async fn write_10(&mut self, data: &[u8], offset: u32, transfer_bytes: u32) {
        let mut buf = [0u8; 31];

        let cmd = Write10Command::new(offset, transfer_bytes, 512).unwrap();
        let wrapper = cmd.wrapper();
        wrapper.push_to_buffer(&mut buf).unwrap();
        cmd.push_to_buffer(&mut buf).unwrap();

        self.msc.bulk_out.request_out(&buf, true).await.unwrap();

        let mut offset = 0;
        while offset < transfer_bytes {
            let chunk_size =
                core::cmp::min(transfer_bytes - offset, BULK_ENDPOINT_PACKET_SIZE as u32);
            self.msc
                .bulk_out
                .request_out(&data[offset as usize..(offset + chunk_size) as usize], true)
                .await
                .unwrap();
            offset += chunk_size;
        }

        let mut csw_buf = [0u8; 13];
        self.msc.bulk_in.request_in(&mut csw_buf).await.unwrap();
        CommandStatusWrapper::pull_from_buffer(&csw_buf).unwrap();
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
    type Error = ();

    async fn read(
        &self,
        blocks: &mut [Block],
        start_block_idx: BlockIdx,
    ) -> Result<(), Self::Error> {
        for (i, block) in blocks.iter_mut().enumerate() {
            self.scsi
                .borrow_mut()
                .read_10(&mut block.contents, start_block_idx.0 + i as u32, 1)
                .await;
        }

        Ok(())
    }

    async fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        for (i, block) in blocks.iter().enumerate() {
            self.scsi
                .borrow_mut()
                .write_10(
                    &block.contents,
                    start_block_idx.0 + i as u32,
                    block.contents.len() as u32,
                )
                .await;
        }

        Ok(())
    }

    async fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        Ok(BlockCount(self.scsi.borrow().size))
    }
}
