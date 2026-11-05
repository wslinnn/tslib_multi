use pyo3::prelude::*;

use crate::types::PyChannel;

/// A hierarchical channel tree.
///
/// Build one from a list of channels or use the server's channel list::
///
///     tree = ChannelTree.from_channels(client.channels())
///     print(tree.print_tree())
#[pyclass(name = "ChannelTree")]
pub struct PyChannelTree {
    inner: tslib_channel::ChannelTree,
}

fn py_channel_to_core(c: &PyChannel) -> tslib_core::state::Channel {
    use tslib_core::MaxClientsKind;

    let max_clients_kind = if c.max_clients < 0 {
        MaxClientsKind::Unlimited
    } else {
        MaxClientsKind::Limited(c.max_clients as u16)
    };
    let max_family_clients_kind = if c.max_family_clients < 0 {
        MaxClientsKind::Unlimited
    } else {
        MaxClientsKind::Limited(c.max_family_clients as u16)
    };

    tslib_core::state::Channel {
        id: c.id,
        parent_id: c.parent_id,
        name: c.name.clone(),
        topic: c.topic.clone(),
        description: c.description.clone(),
        order: c.order,
        is_permanent: c.is_permanent,
        is_semi_permanent: c.is_semi_permanent,
        is_default: c.is_default,
        has_password: c.has_password,
        codec: c.codec,
        codec_quality: c.codec_quality,
        max_clients: c.max_clients,
        max_clients_kind,
        max_family_clients: c.max_family_clients,
        max_family_clients_kind,
        needed_talk_power: c.needed_talk_power,
        icon_id: c.icon_id,
        is_subscribed: c.is_subscribed,
        permission_hints: c.permission_hints,
        permission_hints_known: c.permission_hints_known,
    }
}

#[pymethods]
impl PyChannelTree {
    /// Create an empty channel tree.
    #[new]
    fn new() -> Self {
        Self {
            inner: tslib_channel::ChannelTree::new(),
        }
    }

    /// Build a tree from a list of ``Channel`` objects.
    #[staticmethod]
    fn from_channels(channels: Vec<PyChannel>) -> Self {
        let core_channels: Vec<tslib_core::state::Channel> =
            channels.iter().map(py_channel_to_core).collect();

        Self {
            inner: tslib_channel::ChannelTree::from_channels(core_channels),
        }
    }

    /// Add a channel to the tree.
    fn add_channel(&mut self, channel: PyChannel) {
        self.inner.add_channel(py_channel_to_core(&channel));
    }

    /// Get the root channels (no parent).
    fn roots(&self) -> Vec<PyChannel> {
        self.inner
            .roots()
            .into_iter()
            .map(|c| PyChannel::from(c.clone()))
            .collect()
    }

    /// Get the children of a channel.
    fn children(&self, parent_id: u64) -> Vec<PyChannel> {
        self.inner
            .children(parent_id)
            .into_iter()
            .map(|c| PyChannel::from(c.clone()))
            .collect()
    }

    /// Get the path from root to a channel.
    fn path_to(&self, id: u64) -> Vec<PyChannel> {
        self.inner
            .path_to(id)
            .into_iter()
            .map(|c| PyChannel::from(c.clone()))
            .collect()
    }

    /// Find a channel by name.
    fn find_by_name(&self, name: String) -> Option<PyChannel> {
        self.inner.find_by_name(&name).map(|c| PyChannel::from(c.clone()))
    }

    /// Get all channels as a flat list.
    fn all_channels(&self) -> Vec<PyChannel> {
        self.inner
            .all_channels()
            .into_iter()
            .map(|c| PyChannel::from(c.clone()))
            .collect()
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    /// Return an ASCII representation of the channel tree.
    fn print_tree(&self) -> String {
        self.inner.print_tree()
    }

    fn __repr__(&self) -> String {
        format!("ChannelTree(channels={})", self.inner.len())
    }
}
