use std::fmt::Debug;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::net::UdpSocket;

use trust_dns_client::client::{AsyncClient, ClientHandle};
use trust_dns_client::op::{Message, Edns};
use trust_dns_client::rr::Record;
use trust_dns_client::serialize::binary::{BinDecodable, BinEncodable};
use trust_dns_client::udp::UdpClientStream;

use trust_dns_server::authority::{MessageRequest, MessageResponse, MessageResponseBuilder, Catalog};
use trust_dns_server::server::{Request, RequestHandler, ResponseHandler};

use async_trait::async_trait;

use tracing::instrument;

type RecordBoxedIter<'a> = Box<dyn Iterator<Item=&'a Record> + Send + 'a>;

pub enum QueryStatus<T> {
    Resolved(T),
    Fallthrough,
}

#[async_trait]
pub trait AsyncQueryHandler: Debug + Send + Sync + 'static {
    async fn handle_query(&self, _: &mut Message) -> Result<QueryStatus<Message>>;
}

#[derive(Clone, Debug)]
pub struct DnsProxy {
    handlers: Arc<Vec<Box<dyn AsyncQueryHandler>>>,
}

impl DnsProxy {
    pub fn new(handlers: Vec<Box<dyn AsyncQueryHandler>>) -> Self {
        DnsProxy {
            handlers: Arc::new(handlers),
        }
    }
}

pub trait TryIntoMessage {
    fn try_into_message(&self) -> Result<Message>;
}

impl TryIntoMessage for MessageRequest {
    fn try_into_message(&self) -> Result<Message> {
        let buffer = self.to_bytes()?;
        Ok(BinDecodable::from_bytes(&buffer)?)
    }
}

#[derive(Debug)]
pub struct Upstream {
    upstream: SocketAddr,
    timeout: Duration,
}

impl Upstream {
    pub fn new(upstream: SocketAddr, timeout: Duration) -> Self {
        Upstream { upstream, timeout }
    }
}

#[async_trait]
impl AsyncQueryHandler for Upstream {
    #[instrument(name = "upstream")]
    async fn handle_query(&self, msg: &mut Message) -> Result<QueryStatus<Message>> {
        let conn = UdpClientStream::<UdpSocket>::with_timeout(self.upstream, self.timeout);
        let (mut client, bg) = AsyncClient::connect(conn).await?;
        tokio::spawn(bg);
        let query = msg
            .queries()
            .first()
            .ok_or(anyhow!("empty queries"))?
            .clone();
        let resp =
            client
                .query(
                    query.name().clone(),
                    query.query_class(),
                    query.query_type(),
                )
                .await?;
        Ok(QueryStatus::Resolved(resp.into()))
    }
}

#[async_trait]
impl AsyncQueryHandler for DnsProxy {
    #[instrument(name = "dnsproxy")]
    async fn handle_query(&self, msg: &mut Message) -> Result<QueryStatus<Message>> {
        for handler in self.handlers.iter() {
            match handler.handle_query(msg).await? {
                QueryStatus::Fallthrough => continue,
                result => return Ok(result),
            }
        }
        Ok(QueryStatus::Fallthrough)
    }
}

fn make_response_builder(req: &Request) -> MessageResponseBuilder<'_> {
    let message = &req.message;
    let queries = message.raw_queries();

    MessageResponseBuilder::new(Some(queries))
}

fn make_err_msg_response(req: &Request) -> MessageResponse<'_, 'static> {
    let message = &req.message;
    make_response_builder(req).error_msg(message.id(), message.op_code(), message.response_code())
}

fn make_forward_response<'q, 'a>(
    req: &'q Request,
    resp: &'a Message,
) -> MessageResponse<'q, 'a> {
    let builder = make_response_builder(req);
    let req = &req.message;
    let mut resp = builder.build(
        *resp.header().clone().set_id(req.id()),
        Box::new(resp.answers().iter()) as RecordBoxedIter<'a>,
        Box::new(None.iter()) as RecordBoxedIter<'a>,
        Box::new(None.iter()) as RecordBoxedIter<'a>,
        Box::new(None.iter()) as RecordBoxedIter<'a>,
    );

    if let Some(req_edns) = req.edns() {
        let mut resp_edns = Edns::new();

        // TODO: Check request's edns version against our_version.
        let our_version = 0;
        resp_edns.set_dnssec_ok(true);
        resp_edns.set_max_payload(req_edns.max_payload().max(512));
        resp_edns.set_version(our_version);
        resp.set_edns(resp_edns);
    }
    resp
}

/// This macro trys to handle the `Result` type in `RequestHandler::handle_request`,
/// if the `result` is `Ok(T)`, then the inner value is extracted,
/// otherwise, an error message is sent via `handle` and return from the handler.
macro_rules! try_handle {
    (request = $request:expr, handle = $handle:expr, result = $expr:expr $(,)?) => {
        match $expr {
            std::result::Result::Ok(val) => val,
            std::result::Result::Err(_) => {
                let _ = $handle.send_response(make_err_msg_response(&$request));
                return;
            }
        }
    };
}

impl RequestHandler for DnsProxy {
    type ResponseFuture = Pin<Box<dyn Future<Output=()> + Send>>;
    fn handle_request<R: ResponseHandler>(
        &self,
        request: Request,
        mut response_handle: R,
    ) -> Self::ResponseFuture {
        Catalog::new();
        let clone = self.clone();
        Box::pin(async move {
            let mut message = try_handle!(
                request = request,
                handle = response_handle,
                result = request.message.try_into_message()
            );
            match try_handle!(
                request = request,
                handle = response_handle,
                result = clone.handle_query(&mut message).await
            ) {
                QueryStatus::Fallthrough => {
                    try_handle!(
                        request = request,
                        handle = response_handle,
                        result = Err(())
                    );
                }
                QueryStatus::Resolved(ref result) => {
                    let _ = response_handle.send_response(make_forward_response(&request, result));
                }
            };
        })
    }
}
