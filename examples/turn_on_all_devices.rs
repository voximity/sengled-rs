#[tokio::main]
async fn main() {
    // log in
    let mut client =
        sengled::Client::new("username", "password").with_preferred_qos(sengled::QoS::AtMostOnce);

    client.login_and_start().await.unwrap();

    // get wifi devices
    let devices = client.wifi_devices().await.unwrap();

    // turn all of them on by setting "switch" to "1"
    for mut device in devices {
        device.set_attribute(&client, "switch", "1").await.unwrap();
    }

    // close the client, ensuring MQTT messages are actually sent
    client.close().await.unwrap();
}
