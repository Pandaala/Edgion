//! Rotate one native binding and reject an unresolved extra declaration.
use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use std::time::Duration;
fn allowed() -> Box<dyn TestSuite> {
    Box::new(suites::MessagesBindingAllowedSuite)
}
fn rotated() -> Box<dyn TestSuite> {
    Box::new(suites::MessagesCredentialBSuite)
}
fn rejected() -> Box<dyn TestSuite> {
    Box::new(suites::MessagesBindingRejectedSuite)
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "allowed",
        factory: allowed,
    },
    PhaseSuite {
        name: "rotated",
        factory: rotated,
    },
    PhaseSuite {
        name: "rejected",
        factory: rejected,
    },
];
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let backend = ctx
            .conf_root
            .join("EdgionBackend/Messages/03_EdgionBackend_messages.yaml");
        let secret = ctx.conf_root.join("EdgionBackend/Messages/02_Secret_messages.yaml");
        let original_backend = tokio::fs::read_to_string(&backend).await?;
        let original_secret = tokio::fs::read_to_string(&secret).await?;
        let rotated_secret = original_secret.replace("messages-key-A", "messages-key-B");
        let mut changed: serde_yaml::Value = serde_yaml::from_str(&original_backend)?;
        changed["spec"]["ai"]["credentialPool"]["credentials"]
            .as_sequence_mut()
            .ok_or_else(|| anyhow::anyhow!("missing credentials"))?
            .push(serde_yaml::from_str(
                "name: unresolved\nsecretRef:\n  name: messages-missing-credential\n",
            )?);
        let changed = serde_yaml::to_string(&changed)?;
        let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
        let update = async {
            ctx.wait_phase(SCENARIO, "allowed", Duration::from_secs(15)).await?;
            let socket=ctx.target_socket_addr(ctx.test_ctx.http_port).await?;
            let snapshot_client=reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(15))
                .resolve("messages-provider.example.com",socket).build()?;
            let snapshot=snapshot_client.post(format!("http://messages-provider.example.com:{}/v1/messages?mock_mode=long",ctx.test_ctx.http_port))
                .json(&serde_json::json!({"model":"fixture-model","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"snapshot"}]}))
                .send().await?;
            anyhow::ensure!(snapshot.status().is_success(),"in-flight snapshot did not start");
            crate::dynamic::apply::apply_yaml_documents(&client, &secret, &rotated_secret).await?;
            ctx.wait_phase(SCENARIO, "rotated", Duration::from_secs(15)).await?;
            use futures::StreamExt;
            let mut stream=snapshot.bytes_stream();
            let mut prefix=Vec::new();
            let mut captured=false;
            while let Some(chunk)=stream.next().await {
                prefix.extend_from_slice(&chunk?);
                anyhow::ensure!(prefix.len()<16*1024,"snapshot prefix limit");
                let text=std::str::from_utf8(&prefix)?;
                for line in text.lines().filter_map(|l| l.strip_prefix("data: ")) {
                    let Ok(event)=serde_json::from_str::<serde_json::Value>(line) else {continue};
                    if let Some(text)=event["delta"]["text"].as_str() {
                        if let Ok(observed)=serde_json::from_str::<serde_json::Value>(text) {
                            anyhow::ensure!(observed["credential_label"]=="A","in-flight credential generation changed");
                            captured=true;
                            break;
                        }
                    }
                }
                if captured {break;}
            }
            anyhow::ensure!(captured,"in-flight observation missing");
            drop(stream);

            crate::dynamic::apply::apply_yaml_documents(&client, &secret, &original_secret).await?;
            ctx.wait_phase(SCENARIO, "allowed", Duration::from_secs(15)).await?;
            crate::dynamic::apply::apply_yaml_documents(&client, &backend, &changed).await?;
            ctx.wait_phase(SCENARIO, "rejected", Duration::from_secs(15)).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let restore = async {
            crate::dynamic::apply::apply_yaml_documents(&client, &secret, &original_secret).await?;
            crate::dynamic::apply::apply_yaml_documents(&client, &backend, &original_backend).await?;
            ctx.wait_phase(SCENARIO, "allowed", Duration::from_secs(15)).await
        }
        .await;
        restore?;
        update?;
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "AiMessagesBinding",
    conf_path: "EdgionBackend/Messages/DynamicTest",
    run,
    phases: PHASES,
};
