# sengled-rs

Tiny API wrapper over Sengled smart home devices. Loose port of
[ha-sengledapi](https://github.com/jfarmer08/ha-sengledapi) to Rust.

## Installation

Use `cargo add sengled`.

## Usage

See some [examples](/examples/).

```rs
#[tokio::main]
async fn main() {
    // log in
    let mut client =
        sengled::Client::new("username", "password").with_preferred_qos(sengled::QoS::AtMostOnce);

    client.login_and_start().await.unwrap();

    // get wifi devices
    let devices = client.wifi_devices().await.unwrap();

    // turn all of them on by setting "switch" to "1" and print their names
    for mut device in devices {
        device.set_attribute(&client, "switch", "1").await.unwrap();

        println!("{}", device.get_attribute_or("name", "unknown"));

        // also, maybe explore other attributes to see what else you can do...
        // println!("{:#?}", &device.attributes);
    }

    // close the client, ensuring MQTT messages are actually sent
    client.close().await.unwrap();
}
```

### Optimization

You can use `.with_preferred_qos` on `sengled::Client` to control the MQTT QoS
by which MQTT messages are delivered.

You can use `.with_skip_server_check()` on `sengled::Client` before starting the
client to skip the server check. By default, the wrapper will contact an API
endpoint and gather some information about the target MQTT broker server. In my
testing, this has not changed, so when using `.with_skip_server_check()`, the
wrapper will use constant defaults and save an API call.

You can use `.start(session)` instead of `.login_and_start()` if you already
have the _jsessionId_ for your Sengled account. `.login_and_start()` returns
this _jsessionId_ so you can cache this value and use `.start(session)` later to
save an API call.
