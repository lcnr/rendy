//! CommandPool module docs.

use crate::{buffer::*, capability::*};

/// Simple pool wrapper.
/// Doesn't provide any guarantees.
/// Wraps raw buffers into `CommandCommand buffer`.
#[derive(derivative::Derivative)]
#[derivative(Debug)]
pub struct CommandPool<B: gfx_hal::Backend, C = gfx_hal::QueueType, R = NoIndividualReset> {
    #[derivative(Debug = "ignore")]raw: B::CommandPool,
    capability: C,
    reset: R,
    family: gfx_hal::queue::QueueFamilyId,
    relevant: relevant::Relevant,
}

impl<B, C, R> CommandPool<B, C, R>
where
    B: gfx_hal::Backend,
    R: Reset,
{
    /// Wrap raw command pool.
    ///
    /// # Safety
    ///
    /// * `raw` must be valid command pool handle.
    /// * The command pool must be created for specified `family` index.
    /// * `capability` must be subset of capabilites of the `family` the pool was created for.
    /// * if `reset` is `IndividualReset` the pool must be created with individual command buffer reset flag set.
    pub unsafe fn from_raw(
        raw: B::CommandPool,
        capability: C,
        reset: R,
        family: gfx_hal::queue::QueueFamilyId,
    ) -> Self {
        CommandPool {
            raw,
            capability,
            reset,
            family,
            relevant: relevant::Relevant,
        }
    }

    /// Allocate new command buffers.
    pub fn allocate_buffers<L: Level>(
        &mut self,
        level: L,
        count: usize,
    ) -> Vec<CommandBuffer<B, C, InitialState, L, R>>
    where
        L: Level,
        C: Capability,
    {
        let buffers = gfx_hal::pool::RawCommandPool::allocate(
            &mut self.raw,
            count,
            level.level(),
        );

        buffers
            .into_iter()
            .map(|raw| unsafe {
                CommandBuffer::from_raw(
                    raw,
                    self.capability,
                    InitialState,
                    level,
                    self.reset,
                    self.family,
                )
            }).collect()
    }

    /// Free buffers.
    /// Buffers must be in droppable state.
    /// TODO: Validate buffers were allocated from this pool.
    pub fn free_buffers(
        &mut self,
        buffers: impl IntoIterator<Item = CommandBuffer<'static, B, C, impl Resettable, impl Level, R>>,
    ) {
        let buffers = buffers
            .into_iter()
            .map(|buffer| unsafe { buffer.into_raw() })
            .collect::<Vec<_>>();

        unsafe {
            gfx_hal::pool::RawCommandPool::free(&mut self.raw, buffers);
        }
    }

    /// Reset all buffers of this pool.
    ///
    /// # Safety
    ///
    /// All buffers allocated from this pool must be marked reset.
    /// See [`CommandBuffer::mark_reset`](struct.Command buffer.html#method.mark_reset)
    pub unsafe fn reset(&mut self) {
        gfx_hal::pool::RawCommandPool::reset(&mut self.raw);
    }

    /// Dispose of command pool.
    ///
    /// # Safety
    ///
    /// * All buffers allocated from this pool must be [freed](#method.free_buffers).
    pub unsafe fn dispose(self, device: &impl gfx_hal::Device<B>) {
        device.destroy_command_pool(self.raw);
        self.relevant.dispose();
    }

    /// Convert capability level
    pub fn with_value_capability(self) -> CommandPool<B, gfx_hal::QueueType, R>
    where
        C: Capability,
    {
        CommandPool {
            raw: self.raw,
            capability: self.capability.into_queue_type(),
            reset: self.reset,
            family: self.family,
            relevant: self.relevant,
        }
    }

    /// Convert capability level
    pub fn with_capability<U>(self) -> Result<CommandPool<B, U, R>, Self>
    where
        C: Supports<U>,
    {
        if let Some(capability) = self.capability.supports() {
            Ok(CommandPool {
                raw: self.raw,
                capability,
                reset: self.reset,
                family: self.family,
                relevant: self.relevant,
            })
        } else {
            Err(self)
        }
    }
}

/// Command pool that owns allocated buffers.
/// It can be used to borrow buffers one by one.
/// All buffers will be reset together via pool.
/// Prior resetting user must ensure all buffers are complete.
#[derive(derivative::Derivative)]
#[derivative(Debug)]
pub struct OwningCommandPool<B: gfx_hal::Backend, C = gfx_hal::QueueType, L = PrimaryLevel> {
    inner: CommandPool<B, C>,
    level: L,
    #[derivative(Debug = "ignore")]
    buffers: Vec<B::CommandBuffer>,
    next: usize,
}

impl<B, C, L> OwningCommandPool<B, C, L>
where
    B: gfx_hal::Backend,
{
    /// Wrap simple pool into owning version.
    ///
    /// # Safety
    ///
    /// * All buffers allocated from this pool must be [freed](#method.free_buffers).
    pub unsafe fn from_inner(inner: CommandPool<B, C>, level: L) -> Self {
        OwningCommandPool {
            inner,
            level,
            buffers: Vec::new(),
            next: 0,
        }
    }

    /// Reserve at least `count` buffers.
    /// Allocate if there are not enough unused buffers.
    pub fn reserve(&mut self, count: usize)
    where
        L: Level,
    {
        let total = self.next + count;
        if total >= self.buffers.len() {
            let add = total - self.buffers.len();

            // TODO: avoid Vec allocation.
            self.buffers.extend(
                unsafe {
                    gfx_hal::pool::RawCommandPool::allocate(
                        &mut self.inner.raw,
                        add,
                        self.level.level(),
                    )
                }
            );
        }
    }

    /// Acquire next unused command buffer from pool.
    ///
    /// # Safety
    ///
    /// * Acquired buffer must be [released](struct.Command buffer#method.release) when no longer needed.
    pub fn acquire_buffer(
        &mut self,
    ) -> CommandBuffer<B, C, InitialState, L>
    where
        L: Level,
        C: Capability,
    {
        self.reserve(1);
        self.next += 1;
        unsafe {
            CommandBuffer::from_raw(
                &mut self.buffers[self.next - 1],
                self.inner.capability,
                InitialState,
                self.level,
                self.inner.reset,
                self.inner.family,
            )
        }
    }

    /// Reset all buffers at once.
    /// [`CommandPool::acquire_buffer`](#method.acquire_buffer) will reuse allocated buffers.
    ///
    /// # Safety
    ///
    /// * All buffers acquired from this pool must be released.
    /// * Commands in buffers must be [complete](struct.Command buffer#method.complete).
    ///
    /// Note.
    /// * Any primary buffer that references secondary buffer from this pool will be invalidated.
    pub unsafe fn reset(&mut self) {
        self.inner.reset();
        self.next = 0;
    }

    /// Dispose of command pool.
    ///
    /// # Safety
    ///
    /// Same as for [`CommandPool::reset`](#method.reset).
    pub unsafe fn dispose(mut self, device: &impl gfx_hal::Device<B>) {
        self.reset();
        if !self.buffers.is_empty() {
            gfx_hal::pool::RawCommandPool::free(&mut self.inner.raw, self.buffers);
        }

        self.inner.dispose(device);
    }

    /// Convert capability level.
    pub fn with_value_capability(self) -> OwningCommandPool<B, gfx_hal::QueueType, L>
    where
        C: Capability,
    {
        OwningCommandPool {
            inner: self.inner.with_value_capability(),
            level: self.level,
            buffers: self.buffers,
            next: self.next,
        }
    }

    /// Convert capability level.
    pub fn with_capability<U>(self) -> Result<OwningCommandPool<B, U, L>, Self>
    where
        C: Supports<U>,
    {
        match self.inner.with_capability() {
            Ok(inner) => Ok(OwningCommandPool {
                inner,
                level: self.level,
                buffers: self.buffers,
                next: self.next,
            }),
            Err(inner) => Err(OwningCommandPool {
                inner,
                level: self.level,
                buffers: self.buffers,
                next: self.next,
            })
        }
    }
}
