//! Who is asking for permission.
//! A delegated child runs its tool calls through the parent's permission
//! chain — that inheritance is deliberate (`core/delegation/run.rs`), because a subagent
//! that could write outside the chain would be a way around every rule the user
//! set. But the chain then had no idea it was talking to a child, so an approval
//! prompt raised by three parallel subagents was three identical prompts.
//! The identity travels as a task-local rather than as a parameter. The hook the
//! child inherits is the parent's own `before_tool_call` closure, built once
//! when the session started and holding no per-call agent context; threading a
//! requester through it would mean changing the shape of the hook for every
//! caller, including the main agent that has nothing to put there. Wrapping the
//! child's call in a scope keeps the change where the difference actually is.
//! The scope covers the whole returned future, so it is still in place while the
//! approval dialog is on screen — which is exactly when the identity is needed.

use std::future::Future;

/// The child behind a tool call, when there is one. Absent for the main agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requester {
    /// The star name the user sees (`core/delegation/aliases.rs`).
    pub alias: String,
    /// The type it runs as: `read-only` or `worker`.
    pub agent: String,
}

tokio::task_local! {
    static CURRENT: Requester;
}

/// Runs `work` with `requester` visible to the permission chain.
pub async fn with_requester<F>(requester: Requester, work: F) -> F::Output
where
    F: Future,
{
    CURRENT.scope(requester, work).await
}

/// The child behind the call being evaluated, or `None` for the main agent.
/// Outside a scope — the main agent's own calls, and every test that does not
/// set one — `try_with` fails and the answer is `None`. That is the right
/// default: an unattributed call is the main agent's, and a prompt that named
/// the wrong child would be worse than one that names none.
pub fn current_requester() -> Option<Requester> {
    CURRENT.try_with(Clone::clone).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reports_nothing_outside_a_scope() {
        assert_eq!(current_requester(), None);
    }

    #[tokio::test]
    async fn reports_the_requester_inside_one() {
        let requester = Requester {
            alias: "Vega".to_owned(),
            agent: "read-only".to_owned(),
        };
        let seen = with_requester(requester.clone(), async { current_requester() }).await;
        assert_eq!(seen, Some(requester));
    }

    /// The scope has to survive an await, or it would be gone by the time the
    /// user answers the dialog.
    #[tokio::test]
    async fn survives_an_await_inside_the_scope() {
        let requester = Requester {
            alias: "Wolf 359".to_owned(),
            agent: "worker".to_owned(),
        };
        let seen = with_requester(requester.clone(), async {
            tokio::task::yield_now().await;
            current_requester()
        })
        .await;
        assert_eq!(seen, Some(requester));
    }

    #[tokio::test]
    async fn does_not_leak_out_of_the_scope() {
        let requester = Requester {
            alias: "Rigel".to_owned(),
            agent: "worker".to_owned(),
        };
        with_requester(requester, async {}).await;
        assert_eq!(current_requester(), None);
    }
}
