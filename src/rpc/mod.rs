//! Load-specific RPC add-ons.
//!
//! We keep the wiring minimal: reuse the Ethereum eth API builder and swap in Load engine
//! validator/API builders, mirroring the upstream `RpcAddOns` pattern.

use reth_node_api::FullNodeComponents;
use reth_node_builder::rpc::{
    BasicEngineValidatorBuilder, EngineApiBuilder, EngineValidatorBuilder, EthApiBuilder,
    Identity as RpcIdentity, PayloadValidatorBuilder, RethRpcAddOns, RethRpcMiddleware, RpcAddOns,
};
use reth_node_ethereum::node::EthereumEthApiBuilder;

use crate::engine::{rpc::LoadEngineApiBuilder, validator::LoadEngineValidatorBuilder};

/// Load RPC add-ons wrapper.
#[derive(Debug)]
pub struct LoadAddOns<
    N: FullNodeComponents,
    EthB: EthApiBuilder<N> = EthereumEthApiBuilder,
    PVB = LoadEngineValidatorBuilder,
    EB = LoadEngineApiBuilder<PVB>,
    EVB = BasicEngineValidatorBuilder<PVB>,
    RpcMiddleware = RpcIdentity,
> {
    inner: RpcAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware>,
}

impl<N> Default for LoadAddOns<N, EthereumEthApiBuilder, LoadEngineValidatorBuilder>
where
    N: FullNodeComponents,
    EthereumEthApiBuilder: EthApiBuilder<N>,
{
    fn default() -> Self {
        Self {
            inner: RpcAddOns::new(
                EthereumEthApiBuilder::default(),
                LoadEngineValidatorBuilder::default(),
                LoadEngineApiBuilder::<LoadEngineValidatorBuilder>::default(),
                BasicEngineValidatorBuilder::new(LoadEngineValidatorBuilder::default()),
                RpcIdentity::default(),
            ),
        }
    }
}

impl<N, EthB, PVB, EB, EVB, RpcMiddleware> RethRpcAddOns<N>
    for LoadAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
    PVB: PayloadValidatorBuilder<N>,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcMiddleware: RethRpcMiddleware,
    RpcAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware>: RethRpcAddOns<N>,
{
    type EthApi = <RpcAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware> as RethRpcAddOns<N>>::EthApi;

    fn hooks_mut(
        &mut self,
    ) -> &mut reth_node_builder::rpc::RpcHooks<N, <Self as RethRpcAddOns<N>>::EthApi> {
        self.inner.hooks_mut()
    }
}

impl<N, EthB, PVB, EB, EVB, RpcMiddleware> reth_node_api::NodeAddOns<N>
    for LoadAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
    PVB: PayloadValidatorBuilder<N>,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcMiddleware: RethRpcMiddleware,
    RpcAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware>: reth_node_api::NodeAddOns<N>,
{
    type Handle =
        <RpcAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware> as reth_node_api::NodeAddOns<N>>::Handle;

    fn launch_add_ons(
        self,
        ctx: reth_node_api::AddOnsContext<'_, N>,
    ) -> impl std::future::Future<Output = eyre::Result<Self::Handle>> + Send {
        self.inner.launch_add_ons(ctx)
    }
}
