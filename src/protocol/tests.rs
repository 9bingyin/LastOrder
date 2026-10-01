use super::*;

#[test]
fn room_code_is_random_uuid_and_derivation_is_stable() -> Result<()> {
    let code = RoomCode::generate()?;
    let id = code.id();
    let parsed = RoomCode::parse(&id.to_uppercase())?;
    assert_eq!(id.len(), 36);
    assert_eq!(parsed.id(), id);
    assert_eq!(parsed.capability(), code.capability());
    assert_eq!(
        parsed.discovery_key().public(),
        code.discovery_key().public()
    );
    assert_ne!(RoomCode::generate()?.id(), id);
    let fixed = RoomCode::parse("abcdef00-1234-4abc-8123-abcdef012345")?;
    assert_eq!(
        fixed.capability(),
        "Mwf6Oak-IXGp1RCm-P1BwFSixKaL1fJkFOUKcuGaXoY"
    );
    assert_eq!(
        fixed.discovery_key().public().to_string(),
        "5118857131c986fce31cf9aeb1a652ef166276b87c792db358ec58ab579b2c98"
    );
    assert_ne!(fixed.capability(), code.capability());
    Ok(())
}

#[test]
fn join_input_recognizes_uuid_and_round_trips_room_tickets() -> Result<()> {
    let code = RoomCode::generate()?;
    let endpoint = SecretKey::from_bytes(&[9; 32]).public();
    let address = EndpointAddr::from(endpoint).with_ip_addr("127.0.0.1:3456".parse()?);
    let encoded = RoomTicket::create(&code, address.clone())?;
    assert!(encoded.starts_with("lastorder"));
    assert!(encoded.len() > 36);
    for input in [
        encoded.clone(),
        encoded.to_uppercase(),
        format!("  {encoded}\n"),
    ] {
        let JoinTarget::Ticket {
            code: parsed,
            address: decoded,
        } = JoinTarget::parse(&input)?
        else {
            bail!("没有识别 Ticket")
        };
        assert_eq!(parsed.id(), code.id());
        assert_eq!(decoded, address);
    }
    let JoinTarget::Uuid(parsed) = JoinTarget::parse(&code.id().to_uppercase())? else {
        bail!("没有识别 UUID")
    };
    assert_eq!(parsed.id(), code.id());
    Ok(())
}

#[test]
fn rejects_invalid_room_tickets_and_unsupported_versions() -> Result<()> {
    let code = RoomCode::generate()?;
    let address = EndpointAddr::from(SecretKey::from_bytes(&[7; 32]).public());
    let ticket = RoomTicket { code, address };
    let bytes = ticket.encode_bytes();
    let mut unsupported = bytes.clone();
    unsupported[0] = 2;
    assert!(RoomTicket::decode_bytes(&unsupported).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(RoomTicket::decode_bytes(&trailing).is_err());
    let mut invalid_uuid = bytes.clone();
    invalid_uuid[1..17].fill(0);
    assert!(RoomTicket::decode_bytes(&invalid_uuid).is_err());
    assert!(RoomTicket::decode_bytes(&bytes[..17]).is_err());
    for input in [
        "lastorder!".to_owned(),
        "endpointtest".to_owned(),
        "x".repeat(MAX_JOIN_INPUT + 1),
    ] {
        assert!(JoinTarget::parse(&input).is_err());
    }
    Ok(())
}

#[test]
fn rejects_ticket_address_amplification() -> Result<()> {
    let code = RoomCode::generate()?;
    let address = EndpointAddr::from(SecretKey::from_bytes(&[7; 32]).public())
        .with_addrs((1..=17).map(|port| iroh::TransportAddr::Ip(([127, 0, 0, 1], port).into())));
    assert!(RoomTicket::create(&code, address.clone()).is_err());
    assert!(RoomTicket::decode_bytes(&RoomTicket { code, address }.encode_bytes()).is_err());
    Ok(())
}

#[test]
fn rejects_invalid_join_codes() {
    for code in [
        "",
        "lo1_test",
        "00000000-0000-0000-0000-000000000000",
        "550e8400-e29b-11d4-a716-446655440000",
    ] {
        assert!(RoomCode::parse(code).is_err());
    }
    assert!(RoomCode::parse(&"x".repeat(4097)).is_err());
}

#[test]
fn rejects_empty_or_control_names() {
    for name in ["", "  ", "a\nb"] {
        assert!(validate_name(name).is_err());
    }
}

#[test]
fn quality_defaults_and_bounds_are_validated() -> Result<()> {
    assert_eq!(
        VideoProfile::default(),
        VideoProfile {
            mode: QualityMode::Original,
            height: 1080,
            fps: 30,
            bitrate: 6_000_000
        }
    );
    VideoProfile::default().validate()?;
    for profile in [
        VideoProfile {
            height: 999,
            ..Default::default()
        },
        VideoProfile {
            fps: 0,
            ..Default::default()
        },
        VideoProfile {
            fps: 120,
            ..Default::default()
        },
        VideoProfile {
            bitrate: 31_000_000,
            ..Default::default()
        },
    ] {
        assert!(profile.validate().is_err());
    }
    Ok(())
}

#[test]
fn secrets_require_exact_match() {
    assert!(secret_matches("abc", "abc"));
    assert!(!secret_matches("abc", "abd"));
    assert!(!secret_matches("abc", "ab"));
}
