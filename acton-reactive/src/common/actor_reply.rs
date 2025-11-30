/*
 * Copyright (c) 2024. Govcraft
 *
 * Licensed under either of
 *   * Apache License, Version 2.0 (the "License");
 *     you may not use this file except in compliance with the License.
 *     You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 *   * MIT license: http://opensource.org/licenses/MIT
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the applicable License for the specific language governing permissions and
 * limitations under that License.
 */

//! Convenient helpers for creating handler return types.
//!
//! The [`Reply`] struct provides a namespace for creating the boxed, pinned futures
//! that message handlers need to return.
//!
//! # Infallible Handlers
//!
//! For handlers registered with `mutate_on` or `act_on`:
//!
//! ```ignore
//! // Synchronous handler - no async work needed
//! actor.mutate_on::<MyMessage>(|actor, ctx| {
//!     actor.model.count += 1;
//!     Reply::ready()
//! });
//!
//! // Async handler
//! actor.mutate_on::<MyMessage>(|actor, ctx| {
//!     let handle = actor.handle().clone();
//!     Reply::pending(async move {
//!         handle.send(AnotherMessage).await;
//!     })
//! });
//! ```
//!
//! # Fallible Handlers
//!
//! For handlers registered with `try_mutate_on` or `try_act_on`:
//!
//! ```ignore
//! // Async fallible handler
//! actor.try_mutate_on::<RiskyOp>(|actor, ctx| {
//!     Reply::try_pending(async move {
//!         if bad_condition {
//!             Err(MyError::new("failed"))
//!         } else {
//!             Ok(SuccessResult { value: 42 })
//!         }
//!     })
//! });
//!
//! // Immediate success
//! actor.try_mutate_on::<SimpleOp>(|actor, ctx| {
//!     Reply::try_ok(SuccessResult { value: actor.model.value })
//! });
//!
//! // Immediate error
//! actor.try_mutate_on::<FailingOp>(|actor, ctx| {
//!     Reply::try_err(MyError::new("not supported"))
//! });
//! ```

use std::error::Error;
use std::future::Future;
use std::pin::Pin;

use crate::traits::ActonMessageReply;

/// A utility namespace for creating handler return types.
///
/// Message handlers need to return boxed, pinned futures. This struct provides
/// convenient helper methods to create these return types without boilerplate.
///
/// # Methods
///
/// ## Infallible Handlers (`mutate_on`, `act_on`)
///
/// - [`Reply::ready()`] - For synchronous handlers with no async work
/// - [`Reply::pending()`] - For handlers with async work
///
/// ## Fallible Handlers (`try_mutate_on`, `try_act_on`)
///
/// - [`Reply::try_pending()`] - For async handlers returning `Result`
/// - [`Reply::try_ok()`] - For immediate success
/// - [`Reply::try_err()`] - For immediate error
pub struct Reply;

impl Reply {
    // =========================================================================
    // Infallible handlers (mutate_on, act_on)
    // =========================================================================

    /// Creates an immediately resolving future for synchronous handlers.
    ///
    /// Use this when your handler performs synchronous work and doesn't need
    /// to do any async operations.
    ///
    /// # Example
    ///
    /// ```ignore
    /// actor.mutate_on::<Increment>(|actor, ctx| {
    ///     actor.model.count += ctx.message().0;
    ///     Reply::ready()
    /// });
    /// ```
    #[inline]
    #[must_use]
    pub fn ready() -> Pin<Box<impl Future<Output = ()> + Sized>> {
        Box::pin(async move {})
    }

    /// Wraps an async block into the required handler return type.
    ///
    /// Use this when your handler needs to perform async operations like
    /// sending messages, awaiting I/O, or sleeping.
    ///
    /// # Example
    ///
    /// ```ignore
    /// actor.mutate_on::<StartProcess>(|actor, ctx| {
    ///     let handle = actor.handle().clone();
    ///     Reply::pending(async move {
    ///         handle.send(NextStep).await;
    ///     })
    /// });
    /// ```
    #[inline]
    pub fn pending<F>(future: F) -> Pin<Box<F>>
    where
        F: Future<Output = ()> + Sized,
    {
        Box::pin(future)
    }

    // =========================================================================
    // Fallible handlers (try_mutate_on, try_act_on)
    // =========================================================================

    /// Wraps an async block returning `Result` for fallible handlers.
    ///
    /// Use this when your fallible handler needs to perform async operations
    /// that may succeed or fail.
    ///
    /// # Example
    ///
    /// ```ignore
    /// actor.try_mutate_on::<ProcessPayment>(|actor, ctx| {
    ///     let amount = ctx.message().amount;
    ///     let balance = actor.model.balance;
    ///
    ///     Reply::try_pending(async move {
    ///         if balance < amount {
    ///             Err(InsufficientFunds { balance, required: amount })
    ///         } else {
    ///             Ok(PaymentSuccess { remaining: balance - amount })
    ///         }
    ///     })
    /// });
    /// ```
    #[inline]
    pub fn try_pending<F, T, E>(future: F) -> Pin<Box<F>>
    where
        F: Future<Output = Result<T, E>> + Send + Sync + 'static,
        T: ActonMessageReply + 'static,
        E: Error + Send + Sync + 'static,
    {
        Box::pin(future)
    }

    /// Creates an immediate success result for fallible handlers.
    ///
    /// Use this when your fallible handler can immediately determine success
    /// without async operations.
    ///
    /// # Example
    ///
    /// ```ignore
    /// actor.try_mutate_on::<GetValue>(|actor, ctx| {
    ///     Reply::try_ok(ValueResult { value: actor.model.cached_value })
    /// });
    /// ```
    #[inline]
    #[must_use]
    pub fn try_ok<T, E>(value: T) -> Pin<Box<impl Future<Output = Result<T, E>> + Send + Sync>>
    where
        T: ActonMessageReply + Send + Sync + 'static,
        E: Error + Send + Sync + 'static,
    {
        Box::pin(async move { Ok(value) })
    }

    /// Creates an immediate error result for fallible handlers.
    ///
    /// Use this when your fallible handler can immediately determine failure
    /// without async operations.
    ///
    /// # Example
    ///
    /// ```ignore
    /// actor.try_mutate_on::<UnsupportedOp>(|actor, ctx| {
    ///     Reply::try_err(NotImplementedError::new("operation not supported"))
    /// });
    /// ```
    #[inline]
    #[must_use]
    pub fn try_err<T, E>(error: E) -> Pin<Box<impl Future<Output = Result<T, E>> + Send + Sync>>
    where
        T: ActonMessageReply + Send + Sync + 'static,
        E: Error + Send + Sync + 'static,
    {
        Box::pin(async move { Err(error) })
    }

    // =========================================================================
    // Deprecated aliases for backwards compatibility
    // =========================================================================

    /// Creates an immediately resolving future.
    ///
    /// # Deprecated
    ///
    /// Use [`Reply::ready()`] instead.
    #[inline]
    #[must_use]
    #[deprecated(since = "7.1.0", note = "Use Reply::ready() instead")]
    pub fn immediate() -> Pin<Box<impl Future<Output = ()> + Sized>> {
        Self::ready()
    }

    /// Wraps an async block into a pinned box.
    ///
    /// # Deprecated
    ///
    /// Use [`Reply::pending()`] instead.
    #[inline]
    #[deprecated(since = "7.1.0", note = "Use Reply::pending() instead")]
    pub fn from_async<F>(future: F) -> Pin<Box<F>>
    where
        F: Future<Output = ()> + Sized,
    {
        Self::pending(future)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestSuccess {
        value: i32,
    }

    #[derive(Debug)]
    struct TestError(String);

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl Error for TestError {}

    #[tokio::test]
    async fn test_ready() {
        let fut = Reply::ready();
        fut.await; // Should complete immediately
    }

    #[tokio::test]
    async fn test_pending() {
        let fut = Reply::pending(async {
            // Some async work
        });
        fut.await;
    }

    #[tokio::test]
    async fn test_try_ok() {
        let fut: Pin<Box<dyn Future<Output = Result<TestSuccess, TestError>> + Send + Sync>> =
            Reply::try_ok(TestSuccess { value: 42 });
        let result = fut.await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().value, 42);
    }

    #[tokio::test]
    async fn test_try_err() {
        let fut: Pin<Box<dyn Future<Output = Result<TestSuccess, TestError>> + Send + Sync>> =
            Reply::try_err(TestError("test error".to_string()));
        let result = fut.await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().0, "test error");
    }

    #[tokio::test]
    async fn test_try_pending_ok() {
        let fut =
            Reply::try_pending(async { Ok::<TestSuccess, TestError>(TestSuccess { value: 100 }) });
        let result = fut.await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_try_pending_err() {
        let fut = Reply::try_pending(async { Err::<TestSuccess, _>(TestError("failed".into())) });
        let result = fut.await;
        assert!(result.is_err());
    }
}
