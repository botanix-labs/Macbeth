//! Support for handling notification events for poa

use displaydoc::Display as DisplayDoc;
use errors::SlackClientError;
use hyper_rustls::HttpsConnector;

use hyper_util::client::legacy::connect::HttpConnector;
use slack_morphism::prelude::*;
use thiserror::Error;
use tracing::{error, info};
use url::Url;

/// Event notification errors
#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Slack client error: `{0}`
    SlackClient(#[from] SlackClientError),
    /// Io error: `{0}`
    Io(std::io::Error),
}

/// Events Notification Client
#[derive(Clone, Debug)]
pub struct EventsNotificationClient {
    client_id: secp256k1::PublicKey,
    client: SlackClient<SlackClientHyperConnector<HttpsConnector<HttpConnector>>>,
    webhook_url: Url,
}

impl EventsNotificationClient {
    /// Client builder
    pub fn new(client_id: secp256k1::PublicKey, webhook_url: Url) -> Result<Self, Error> {
        let client = SlackClient::new(SlackClientHyperConnector::new().map_err(Error::Io)?);
        Ok(Self { client_id, client, webhook_url })
    }

    /// Getter for the selected webhook url
    #[inline]
    pub fn get_webhook_url(&self) -> &Url {
        &self.webhook_url
    }

    /// Client id
    #[inline]
    pub fn get_client_id(&self) -> &secp256k1::PublicKey {
        &self.client_id
    }

    /// Getter for the client sender
    #[inline]
    pub fn get_client(
        &self,
    ) -> &SlackClient<SlackClientHyperConnector<HttpsConnector<HttpConnector>>> {
        &self.client
    }

    /// Sends a notification message to the webhook url
    pub async fn send_message(
        &self,
        message: &str,
    ) -> Result<SlackApiPostWebhookMessageResponse, Error> {
        let msg = format!("[CLIENT_ID = {}]. Text = {}", self.client_id, message);
        let client_result = self
            .client
            .post_webhook_message(
                &self.webhook_url,
                &SlackApiPostWebhookMessageRequest::new(SlackMessageContent::new().with_text(msg)),
            )
            .await
            .map_err(Error::SlackClient)?;

        Ok(client_result)
    }
}

/// Sends a notification using the client
pub async fn send_slack_notification(
    slack_client: Option<EventsNotificationClient>,
    message: &str,
) {
    if let Some(slack_client) = slack_client.as_ref() {
        match slack_client.send_message(message).await {
            Ok(_) => info!(">>> Notification fired!"),
            Err(err) => error!(">>> Notification client failed to send. Error = {:?}", err),
        }
    }
}
