use embassy_usb::handlers::{StaticHandlerSpec, UsbHostHandler};
use embassy_usb::host::descriptor::{InterfaceDescriptor, USBDescriptor};
use embassy_usb_driver::host::{UsbChannel, UsbHostDriver, channel};
use embassy_usb_driver::{Direction, EndpointInfo, EndpointType};
use scsi::BufferPushable;

#[derive(Debug, defmt::Format)]
pub enum MscEvent {}

/// Host side driver for Mass Storage Class
pub struct MscHandler<H: UsbHostDriver> {
    pub bulk_in: H::Channel<channel::Bulk, channel::In>,
    pub bulk_out: H::Channel<channel::Bulk, channel::Out>,
    _control: H::Channel<channel::Control, channel::InOut>,
}

impl<H: UsbHostDriver> UsbHostHandler for MscHandler<H> {
    type PollEvent = MscEvent;
    type Driver = H;

    fn static_spec() -> embassy_usb::handlers::StaticHandlerSpec {
        StaticHandlerSpec {
            device_filter: None,
        }
    }

    async fn try_register(
        bus: &Self::Driver,
        enum_info: &embassy_usb::handlers::EnumerationInfo,
    ) -> Result<Self, embassy_usb::handlers::RegisterError> {
        let mut control = bus.alloc_channel::<channel::Control, channel::InOut>(
            enum_info.device_address,
            &EndpointInfo::new(
                0.into(),
                EndpointType::Control,
                (enum_info.device_desc.max_packet_size0 as u16)
                    .min(enum_info.speed.max_packet_size()),
            ),
            enum_info.ls_over_fs,
        )?;

        let mut cfg_desc_buf = [0u8; 512];
        let configuration = enum_info
            .active_config_or_set_default(&mut control, &mut cfg_desc_buf)
            .await?;

        let iface = configuration
            .iter_interface()
            .find(|v| {
                matches!(
                    v,
                    InterfaceDescriptor {
                        interface_class: 0x08,
                        interface_subclass: 0x06,
                        interface_protocol: 0x50,
                        ..
                    }
                )
            })
            .ok_or(embassy_usb::handlers::RegisterError::NoSupportedInterface)?;

        let bulk_in_ep = iface
            .iter_endpoints()
            .find(|v| v.ep_type() == EndpointType::Bulk && v.ep_dir() == Direction::In)
            .ok_or(embassy_usb::handlers::RegisterError::NoSupportedInterface)?;

        let bulk_out_ep = iface
            .iter_endpoints()
            .find(|v| v.ep_type() == EndpointType::Bulk && v.ep_dir() == Direction::Out)
            .ok_or(embassy_usb::handlers::RegisterError::NoSupportedInterface)?;

        configuration.set_configuration(&mut control).await?;

        let bulk_in = bus.alloc_channel::<channel::Bulk, channel::In>(
            enum_info.device_address,
            &bulk_in_ep.into(),
            enum_info.ls_over_fs,
        )?;

        let bulk_out = bus.alloc_channel::<channel::Bulk, channel::Out>(
            enum_info.device_address,
            &bulk_out_ep.into(),
            enum_info.ls_over_fs,
        )?;

        Ok(Self {
            bulk_in,
            bulk_out,
            _control: control,
        })
    }

    async fn wait_for_event(
        &mut self,
    ) -> Result<
        embassy_usb::handlers::HandlerEvent<Self::PollEvent>,
        embassy_usb_driver::host::HostError,
    > {
        Ok(embassy_usb::handlers::HandlerEvent::NoChange)
    }
}
