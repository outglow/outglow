use super::Gpd;
use crate::util::Reader;

#[test]
fn a_device_address_is_read_for_the_source_id_and_ieee_application_ids_only() {
    let b = [0x45, 0x23, 0x71, 0x01, 0x11, 0x22, 0x33, 0x44, 0x07];
    assert!(Gpd::read(0, &mut Reader::new(&b)) == Some((Gpd::Src(0x0171_2345), 0)));
    assert!(Gpd::read(2, &mut Reader::new(&b)) == Some((Gpd::Ieee(0x4433_2211_0171_2345), 0x07)));
    for app in [1, 3, 7] {
        assert!(Gpd::read(app, &mut Reader::new(&b)).is_none(), "{app}");
    }
}
