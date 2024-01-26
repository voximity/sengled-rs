#[tokio::main]
async fn main() {
    // log in
    let mut client =
        sengled::Client::new("username", "password").with_preferred_qos(sengled::QoS::AtMostOnce);

    client.login().await.unwrap();
    let event_handler = client.start().await.unwrap();

    // we must spawn the event handler listener for the API to function, or handle the events ourselves
    event_handler.spawn_listener(&mut client);

    // get wifi devices
    let devices = client.wifi_devices().await.unwrap();

    // turn all of them on by setting "switch" to "1"
    for device in devices {
        client
            .set_device_attribute(device, "switch", "1")
            .await
            .unwrap();
    }

    // close the client, ensuring MQTT messages are actually sent
    client.close().await.unwrap();
}
