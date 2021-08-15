use std::fmt::Debug;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::net::UdpSocket;

use trust_dns_client::client::{AsyncClient, ClientHandle};
use trust_dns_client::op::{Edns, Message};
use trust_dns_client::rr::Record;
use trust_dns_client::serialize::binary::{BinDecodable, BinEncodable};
use trust_dns_client::udp::UdpClientStream;

use trust_dns_server::authority::{MessageRequest, MessageResponse, MessageResponseBuilder};
use trust_dns_server::server::{Request, RequestHandler, ResponseHandler};

use async_trait::async_trait;

use tracing::{info_span, instrument, Instrument};

type RecordBoxedIter<'a> = Box<dyn Iterator<Item = &'a Record> + Send + 'a>;
#[async_trait]
pub trait AsyncQueryHandler: Debug + Send + Sync + 'static {
    fn with_next(self: Box<Self>, next: Box<dyn AsyncQueryHandler>) -> Box<dyn AsyncQueryHandler>;
    fn next(&self) -> Option<&Box<dyn AsyncQueryHandler>>;

    async fn next_handler(&self, msg: Message) -> Result<Message> {
        if let Some(ref handler) = self.next() {
            return handler.handle_query(msg).await;
        }
        Ok(msg)
    }
    async fn handle_query(&self, _: Message) -> Result<Message>;
}

#[derive(Clone)]
pub struct DnsProxy {
    handlers: Arc<dyn AsyncQueryHandler>,
}

impl DnsProxy {
    pub fn new(handlers: Vec<Box<dyn AsyncQueryHandler>>) -> Result<Self> {
        let mut first = None;
        for handler in handlers.into_iter().rev() {
            first = match first {
                None => Some(handler),
                Some(head) => Some(head.with_next(handler)),
            }
        }

        Ok(DnsProxy {
            handlers: Arc::from(first.ok_or_else(|| anyhow!("empty handlers chain"))?),
        })
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
    next: Option<Box<dyn AsyncQueryHandler>>,
}

impl Upstream {
    pub fn new(upstream: SocketAddr, timeout: Duration) -> Self {
        Upstream {
            upstream,
            timeout,
            next: None,
        }
    }
}

#[async_trait]
impl AsyncQueryHandler for Upstream {
    fn with_next(
        mut self: Box<Self>,
        next: Box<dyn AsyncQueryHandler>,
    ) -> Box<dyn AsyncQueryHandler> {
        self.next = Some(next);
        self
    }

    fn next(&self) -> Option<&Box<dyn AsyncQueryHandler>> {
        self.next.as_ref()
    }

    #[instrument(name = "upstream")]
    async fn handle_query(&self, msg: Message) -> Result<Message> {
        let conn = UdpClientStream::<UdpSocket>::with_timeout(self.upstream, self.timeout);
        let (mut client, bg) = AsyncClient::connect(conn).await?;
        tokio::spawn(bg);
        let query = msg
            .queries()
            .first()
            .ok_or(anyhow!("empty queries"))?
            .clone();
        let resp = client
            .query(
                query.name().clone(),
                query.query_class(),
                query.query_type(),
            )
            .await?;

        self.next_handler(resp.into()).await
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

fn make_forward_response<'q, 'a>(req: &'q Request, resp: &'a Message) -> MessageResponse<'q, 'a> {
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
    type ResponseFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
    fn handle_request<R: ResponseHandler>(
        &self,
        request: Request,
        mut response_handle: R,
    ) -> Self::ResponseFuture {
        let clone = self.clone();
        Box::pin(
            (async move {
                let message = try_handle!(
                    request = request,
                    handle = response_handle,
                    result = request.message.try_into_message()
                );
                let resp = try_handle!(
                    request = request,
                    handle = response_handle,
                    result = clone.handlers.handle_query(message).await
                );
                let _ = response_handle.send_response(make_forward_response(&request, &resp));
            })
            .instrument(info_span!("handle_request")),
        )
    }
}
