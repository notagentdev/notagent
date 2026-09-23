use super::*;

impl InteractiveMode {
    // ------------------------------------------------------------------
    // Session events
    // ------------------------------------------------------------------

    pub(super) async fn handle_event(&mut self, event: AgentSessionEvent) {
        self.footer.borrow_mut().invalidate();

        match event {
            AgentSessionEvent::Agent(AgentEvent::AgentStart) => {
                // A previous run that ended without AgentEnd left its rows open.
                self.settle_unfinished_entries();
                // The retry's success event arrives later, but Escape has to
                // abort the run again from here on.
                if self.escape_target == EscapeTarget::Retry {
                    self.escape_target = EscapeTarget::Default;
                }
                if self.settings().get_show_terminal_progress() {
                    self.ui
                        .with_terminal(|terminal| terminal.set_progress(true));
                }
                if self.working_visible {
                    let message = self
                        .working_message
                        .clone()
                        .unwrap_or_else(|| DEFAULT_WORKING_MESSAGE.to_owned());
                    self.show_status_indicator(StatusIndicator::working(message, None));
                } else {
                    self.clear_status_indicator(None);
                }
                self.ui.request_render();
            }
            AgentSessionEvent::Agent(AgentEvent::MessageStart { message }) => match message {
                AgentMessage::Assistant(message) => {
                    let component = Rc::new(RefCell::new(AssistantMessageComponent::new(
                        None,
                        self.hide_thinking_block,
                        Some(get_markdown_theme()),
                        Some(self.hidden_thinking_label.clone()),
                        Some(self.output_pad),
                        Vec::new(),
                    )));
                    component
                        .borrow_mut()
                        .set_regular_streaming(self.cell.mode() == TuiMode::Regular);
                    self.chat_container
                        .borrow_mut()
                        .add_child(Rc::clone(&component) as ComponentRef);
                    self.chat_container
                        .borrow_mut()
                        .mark_mutable(&(Rc::clone(&component) as ComponentRef));
                    component
                        .borrow_mut()
                        .update_content(message.clone(), Some(true));
                    self.chat_container
                        .borrow_mut()
                        .mark_changed(&(Rc::clone(&component) as ComponentRef));
                    component
                        .borrow_mut()
                        .set_expanded(self.tool_output_expanded);
                    self.chat_expandables
                        .push(Rc::clone(&component) as Rc<RefCell<dyn Expandable>>);
                    self.streaming_component = Some(component);
                    // Providers may deliver the first visible content with
                    // MessageStart rather than a later delta.
                    self.streaming_visible_chars = streamed_visible_chars(&message);
                    if self.streaming_visible_chars > 0 {
                        self.close_explore_block();
                    }
                    self.streaming_message = Some(message);
                    self.ui.request_render();
                }
                message => {
                    self.add_message_to_chat(&message, false);
                    self.ui.request_render();
                }
            },
            AgentSessionEvent::Agent(AgentEvent::MessageUpdate {
                message,
                assistant_message_event,
            }) => {
                if let (Some(component), AgentMessage::Assistant(message)) =
                    (self.streaming_component.clone(), message)
                {
                    let delta = match &assistant_message_event {
                        notagent_ai::types::AssistantMessageEvent::TextDelta {
                            content_index,
                            delta,
                            ..
                        } => Some((*content_index, delta.as_str(), false)),
                        notagent_ai::types::AssistantMessageEvent::ThinkingDelta {
                            content_index,
                            delta,
                            ..
                        } => Some((*content_index, delta.as_str(), true)),
                        _ => None,
                    };
                    let appended = delta.is_some_and(|(index, text, thinking)| {
                        component
                            .borrow_mut()
                            .append_stream_delta(index, text, thinking)
                    });
                    if !appended {
                        component
                            .borrow_mut()
                            .update_content(message.clone(), Some(true));
                    }
                    if component.borrow_mut().take_stream_reflow() {
                        self.rebuild_regular_view();
                    }
                    self.chat_container
                        .borrow_mut()
                        .mark_changed(&(Rc::clone(&component) as ComponentRef));
                    // Settle the preceding block before any subsequent calls
                    // open their own exploration below this text or thought.
                    let visible = if appended {
                        self.streaming_visible_chars
                            + usize::from(delta.is_some_and(|(_, text, _)| !text.trim().is_empty()))
                    } else {
                        streamed_visible_chars(&message)
                    };
                    if visible > self.streaming_visible_chars {
                        self.close_explore_block();
                    }
                    self.streaming_visible_chars = visible;
                    if !appended {
                        for content in &message.content {
                            if let notagent_ai::types::AssistantContent::ToolCall(call) = content {
                                let args = serde_json::Value::Object(call.arguments.clone());
                                // A streamed bash call does not reveal whether it
                                // is background work until its later arguments
                                // arrive. Wait for ToolExecutionStart, where the
                                // complete arguments let us omit background bash
                                // without ever painting a transient tool row.
                                if call.name == "bash" {
                                    self.remove_tool_component(&call.id);
                                    continue;
                                }
                                match self.tool_component(&call.id) {
                                    Some(component) => {
                                        component.borrow_mut().update_args(args);
                                        self.chat_container
                                            .borrow_mut()
                                            .mark_changed(&(component as ComponentRef));
                                    }
                                    None => self.add_tool_component(&call.name, &call.id, args),
                                }
                            }
                        }
                    }
                    self.streaming_message = Some(message);
                    self.ui.request_render();
                }
            }
            AgentSessionEvent::Agent(AgentEvent::MessageEnd { message }) => {
                if let AgentMessage::Assistant(mut message) = message
                    && let Some(component) = self.streaming_component.clone()
                {
                    let mut error_message: Option<String> = None;
                    if message.stop_reason == StopReason::Aborted {
                        let attempt = self.session().retry_attempt();
                        let text = if attempt > 0 {
                            format!(
                                "Aborted after {attempt} retry attempt{}",
                                if attempt > 1 { "s" } else { "" }
                            )
                        } else {
                            "Operation aborted".to_owned()
                        };
                        message.error_message = Some(text.clone());
                        error_message = Some(text);
                    }
                    component
                        .borrow_mut()
                        .update_content(message.clone(), Some(false));
                    self.chat_container
                        .borrow_mut()
                        .mark_stable(&(Rc::clone(&component) as ComponentRef));

                    if matches!(message.stop_reason, StopReason::Aborted | StopReason::Error) {
                        let error_message = error_message.unwrap_or_else(|| {
                            message
                                .error_message
                                .clone()
                                .unwrap_or_else(|| "Error".to_owned())
                        });
                        for (_, component) in std::mem::take(&mut self.pending_tools) {
                            component
                                .borrow_mut()
                                .update_result(error_result(&error_message), false);
                            self.chat_container
                                .borrow_mut()
                                .mark_stable(&(component as ComponentRef));
                        }
                        self.pending_tool_by_call.clear();
                    } else {
                        // Args are complete: the edit tools compute their diff.
                        for (_, component) in self.pending_tools.iter() {
                            component.borrow_mut().set_args_complete();
                        }
                        self.maybe_show_cache_miss_notice(&message);
                    }
                    // An aborted or failed turn settles the block red: the
                    // exploration was cut off, so it must not read as one that
                    // went fine. Visible assistant text ends the run too —
                    // unless this very message announced the exploration its
                    // text introduces.
                    if matches!(message.stop_reason, StopReason::Aborted | StopReason::Error) {
                        self.abort_explore_block();
                    } else if assistant_message_ends_search_run(&message) {
                        self.close_explore_block();
                    }
                    self.streaming_component = None;
                    self.streaming_message = None;
                    self.streaming_visible_chars = 0;
                    self.footer.borrow_mut().invalidate();
                }
                self.ui.request_render();
            }
            AgentSessionEvent::Agent(AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            }) => {
                if is_background_bash_call(&tool_name, &args) {
                    self.remove_tool_component(&tool_call_id);
                    self.ui.request_render();
                    return;
                }
                if self.tool_component(&tool_call_id).is_none() {
                    self.add_tool_component(&tool_name, &tool_call_id, args);
                }
                if let Some(component) = self.tool_component(&tool_call_id) {
                    component.borrow_mut().mark_execution_started();
                    self.chat_container
                        .borrow_mut()
                        .mark_changed(&(component as ComponentRef));
                } else if let Some(block) = self.explore_block_by_call.get(&tool_call_id).cloned() {
                    let entry = Rc::clone(&block) as ComponentRef;
                    if !self.chat_container.borrow().is_stable(&entry) {
                        block.borrow_mut().mark_execution_started(&tool_call_id);
                        self.chat_container.borrow_mut().mark_changed(&entry);
                    }
                }
                self.ui.request_render();
            }
            AgentSessionEvent::Agent(AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            }) => {
                if let Some(component) = self.tool_component(&tool_call_id) {
                    component
                        .borrow_mut()
                        .update_result(tool_result(partial_result, false), true);
                    self.chat_container
                        .borrow_mut()
                        .mark_changed(&(component as ComponentRef));
                    self.ui.request_render();
                }
            }
            AgentSessionEvent::Agent(AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            }) => {
                let todo_details = (tool_name == "todo_write")
                    .then(|| result.details.clone())
                    .flatten();
                if let Some(component) = self.tool_component(&tool_call_id) {
                    component
                        .borrow_mut()
                        .update_result(tool_result(result, is_error), false);
                    self.pending_tools.retain(|(id, _)| id != &tool_call_id);
                    self.pending_tool_by_call.remove(&tool_call_id);
                    self.chat_container
                        .borrow_mut()
                        .mark_stable(&(component as ComponentRef));
                    self.ui.request_render();
                } else if let Some((name, args)) = self.settled_tool_calls.remove(&tool_call_id) {
                    // The turn settled this call's row as failed; the late
                    // result gets a row of its own below.
                    let component = self.create_tool_component(&name, &tool_call_id, args);
                    {
                        let mut row = component.borrow_mut();
                        row.set_expanded(self.tool_output_expanded);
                        row.mark_execution_started();
                        row.update_result(tool_result(result, is_error), false);
                    }
                    self.chat_container
                        .borrow_mut()
                        .add_child(Rc::clone(&component) as ComponentRef);
                    self.chat_tool_rows.push(Rc::clone(&component));
                    self.chat_expandables
                        .push(component as Rc<RefCell<dyn Expandable>>);
                    self.ui.request_render();
                } else if is_explore_tool(&tool_name) {
                    // Search calls live in a block, not in a tool row; the
                    // block may already be closed, so route by call id.
                    if let Some(block) = self.explore_block_by_call.get(&tool_call_id).cloned() {
                        let entry = Rc::clone(&block) as ComponentRef;
                        if self.chat_container.borrow().is_stable(&entry) {
                            self.append_detached_explore_result(&block, &tool_call_id, is_error);
                        } else {
                            block.borrow_mut().complete_call(&tool_call_id, is_error);
                            self.chat_container.borrow_mut().mark_changed(&entry);
                            if !block.borrow().is_running() {
                                self.chat_container.borrow_mut().mark_stable(&entry);
                            }
                        }
                        self.ui.request_render();
                    }
                }
                // Fed from the result rather than from the store: a list whose
                // items are all completed is gone from the store by now.
                if tool_name == "todo_write" {
                    let todos = todo_details
                        .as_ref()
                        .and_then(|details| details.get("after"))
                        .and_then(|after| serde_json::from_value::<Vec<Todo>>(after.clone()).ok())
                        .unwrap_or_default();
                    self.sync_todo_panel(todos, true);
                    self.ui.request_render();
                }
            }
            AgentSessionEvent::AgentEnd { .. } => {
                // The run is over, so the exploration is too — the reference
                // closes on `TaskComplete` for the same reason. Without this a
                // later turn would hang its first calls on the stale block.
                self.close_explore_block();
                if self.settings().get_show_terminal_progress() {
                    self.ui
                        .with_terminal(|terminal| terminal.set_progress(false));
                }
                self.settle_status_indicator(StatusIndicatorKind::Working);
                if let Some(component) = self.streaming_component.take() {
                    self.chat_container
                        .borrow_mut()
                        .remove_child(&(component as ComponentRef));
                    self.streaming_message = None;
                }
                self.settle_unfinished_entries();
                self.ui.request_render();
            }
            AgentSessionEvent::QueueUpdate { .. } => {
                self.update_pending_messages_display();
                self.ui.request_render();
            }
            AgentSessionEvent::CompactionStart { reason } => {
                if self.settings().get_show_terminal_progress() {
                    self.ui
                        .with_terminal(|terminal| terminal.set_progress(true));
                }
                // The editor stays live; submissions are queued while it runs.
                self.escape_target = EscapeTarget::Compaction;
                self.show_compaction_chat_indicator(compaction_status_reason(reason));
                self.ui.request_render();
            }
            AgentSessionEvent::CompactionEnd {
                reason,
                result,
                aborted,
                will_retry,
                error_message,
            } => {
                let resumes = will_retry && !aborted && result.is_some();
                if self.settings().get_show_terminal_progress() {
                    self.ui
                        .with_terminal(|terminal| terminal.set_progress(resumes));
                }
                if self.escape_target == EscapeTarget::Compaction {
                    self.escape_target = EscapeTarget::Default;
                }
                self.clear_compaction_chat_indicator();
                if aborted {
                    if reason == CompactionReason::Manual {
                        self.show_error("Compaction cancelled");
                    } else {
                        self.show_status("Auto-compaction cancelled");
                    }
                } else if let Some(result) = result {
                    self.add_message_to_chat(
                        &AgentMessage::CompactionSummary(create_compaction_summary_message(
                            &result.summary,
                            result.tokens_before,
                            result.estimated_tokens_after,
                            now_millis(),
                        )),
                        false,
                    );
                    self.footer.borrow_mut().invalidate();
                } else if let Some(error_message) = error_message {
                    if reason == CompactionReason::Manual {
                        self.show_error(&error_message);
                    } else {
                        let mut chat = self.chat_container.borrow_mut();
                        chat.add_child(component_ref(Spacer::new(1)));
                        chat.add_child(component_ref(Text::new(
                            theme().fg(ThemeColor::Error, &error_message),
                            1,
                            0,
                        )));
                    }
                }
                if resumes && self.working_visible {
                    let message = self
                        .working_message
                        .clone()
                        .unwrap_or_else(|| DEFAULT_WORKING_MESSAGE.to_owned());
                    self.show_status_indicator(StatusIndicator::working(message, None));
                }
                self.flush_compaction_queue(will_retry).await;
                self.ui.request_render();
            }
            AgentSessionEvent::AutoRetryStart {
                attempt,
                max_attempts,
                delay_ms,
                ..
            } => {
                self.escape_target = EscapeTarget::Retry;
                self.show_status_indicator(StatusIndicator::retry(
                    attempt as u32,
                    max_attempts as u32,
                    delay_ms,
                ));
                self.ui.request_render();
            }
            AgentSessionEvent::AutoRetryEnd {
                attempt,
                success,
                final_error,
                ..
            } => {
                if self.escape_target == EscapeTarget::Retry {
                    self.escape_target = EscapeTarget::Default;
                }
                self.clear_status_indicator(Some(StatusIndicatorKind::Retry));
                if !success {
                    self.show_error(&format!(
                        "Retry failed after {attempt} attempts: {}",
                        final_error.unwrap_or_else(|| "Unknown error".to_owned())
                    ));
                }
                self.ui.request_render();
            }
            AgentSessionEvent::PersistenceError { error_message } => {
                self.show_error(&error_message);
                self.ui.request_render();
            }
            AgentSessionEvent::SessionInfoChanged { .. } => {
                self.update_terminal_title();
                self.footer.borrow_mut().invalidate();
                self.ui.request_render();
            }
            AgentSessionEvent::ThinkingLevelChanged { .. } => {
                self.footer.borrow_mut().invalidate();
                self.update_editor_border_color();
            }
            AgentSessionEvent::EntryAppended { entry } => {
                if let Some(record) = TaskLifecycleRecord::from_session_entry(&entry) {
                    self.append_task_lifecycle(&record);
                }
            }
            // Other session bookkeeping arrives with the slices that own it.
            _ => {}
        }
    }

    pub(super) fn tool_component(
        &self,
        tool_call_id: &str,
    ) -> Option<Rc<RefCell<ToolExecutionComponent>>> {
        self.pending_tool_by_call.get(tool_call_id).cloned()
    }

    pub(super) fn remove_tool_component(&mut self, tool_call_id: &str) {
        let Some(index) = self
            .pending_tools
            .iter()
            .position(|(id, _)| id == tool_call_id)
        else {
            return;
        };
        let (_, component) = self.pending_tools.remove(index);
        self.pending_tool_by_call.remove(tool_call_id);
        let child = Rc::clone(&component) as ComponentRef;
        self.chat_container.borrow_mut().remove_child(&child);
        self.chat_tool_rows
            .retain(|candidate| !Rc::ptr_eq(candidate, &component));
        let expandable = component as Rc<RefCell<dyn Expandable>>;
        self.chat_expandables
            .retain(|candidate| !Rc::ptr_eq(candidate, &expandable));
    }

    pub(super) fn add_tool_component(
        &mut self,
        tool_name: &str,
        tool_call_id: &str,
        args: serde_json::Value,
    ) {
        // `todo_write` already has the dock panel. A background bash and every
        // `task` call are represented by immutable lifecycle rows; keeping the
        // ordinary tool component would stream a second copy into the
        // transcript (subagents: user decision 2026-08-31).
        if tool_name == "todo_write"
            || tool_name == "task"
            || is_background_bash_call(tool_name, &args)
        {
            return;
        }
        if is_explore_tool(tool_name) {
            // Streaming repeats earlier calls even after another tool closes
            // their block. Keep each call in its original block so its result
            // cannot leave a duplicate permanently pending.
            let existing = self.explore_block_by_call.get(tool_call_id).cloned();
            if let Some(block) = &existing
                && self
                    .chat_container
                    .borrow()
                    .is_stable(&(Rc::clone(block) as ComponentRef))
            {
                // The settled block already shows this call and may be in
                // native scrollback.
                return;
            }
            let block = existing.unwrap_or_else(|| self.open_explore_block(false));
            block
                .borrow_mut()
                .push_call(tool_name, tool_call_id.to_owned(), &args);
            self.explore_block_by_call
                .insert(tool_call_id.to_owned(), Rc::clone(&block));
            self.chat_container
                .borrow_mut()
                .mark_changed(&(block as ComponentRef));
            return;
        }
        // Any other tool row ends the run of searches.
        self.close_explore_block();
        let component = self.create_tool_component(tool_name, tool_call_id, args);
        component
            .borrow_mut()
            .set_expanded(self.tool_output_expanded);
        self.chat_container
            .borrow_mut()
            .add_child(Rc::clone(&component) as ComponentRef);
        self.chat_container
            .borrow_mut()
            .mark_mutable(&(Rc::clone(&component) as ComponentRef));
        self.chat_tool_rows.push(Rc::clone(&component));
        self.chat_expandables
            .push(Rc::clone(&component) as Rc<RefCell<dyn Expandable>>);
        self.pending_tool_by_call
            .insert(tool_call_id.to_owned(), Rc::clone(&component));
        self.pending_tools
            .push((tool_call_id.to_owned(), component));
    }

    /// Leaves nothing mutable behind when a run ends. A mutable entry holds
    /// every later row out of native scrollback, so a row still waiting for
    /// its result settles as failed; should the result arrive after all, it
    /// gets a row of its own. Rows restored from the session file are already
    /// settled and stay as they are.
    pub(super) fn settle_unfinished_entries(&mut self) {
        self.close_explore_block();
        self.pending_tool_by_call.clear();
        for (tool_call_id, component) in std::mem::take(&mut self.pending_tools) {
            let entry = Rc::clone(&component) as ComponentRef;
            if self.chat_container.borrow().is_stable(&entry) {
                continue;
            }
            {
                let mut row = component.borrow_mut();
                self.settled_tool_calls.insert(
                    tool_call_id,
                    (row.tool_name().to_owned(), row.args().clone()),
                );
                row.update_result(error_result("The run ended without a result"), false);
            }
            let mut chat = self.chat_container.borrow_mut();
            chat.mark_changed(&entry);
            chat.mark_stable(&entry);
        }
        for block in &self.chat_explore_blocks {
            let entry = Rc::clone(block) as ComponentRef;
            if self.chat_container.borrow().is_stable(&entry) || !block.borrow().is_running() {
                continue;
            }
            {
                let mut block = block.borrow_mut();
                block.fail_running_calls();
                block.close();
            }
            let mut chat = self.chat_container.borrow_mut();
            chat.mark_changed(&entry);
            chat.mark_stable(&entry);
        }
    }

    /// A search result for a block that already settled: the block may be in
    /// native scrollback, so the result is appended as a block of its own.
    fn append_detached_explore_result(
        &mut self,
        block: &Rc<RefCell<ExploreBlockComponent>>,
        tool_call_id: &str,
        failed: bool,
    ) {
        let Some(detached) = block.borrow().detached_result(tool_call_id, failed) else {
            return;
        };
        let detached = Rc::new(RefCell::new(detached));
        self.chat_container
            .borrow_mut()
            .add_child(Rc::clone(&detached) as ComponentRef);
        self.chat_expandables
            .push(Rc::clone(&detached) as Rc<RefCell<dyn Expandable>>);
        self.chat_explore_blocks.push(Rc::clone(&detached));
        self.explore_block_by_call
            .insert(tool_call_id.to_owned(), detached);
    }

    /// Ends the run of consecutive searches: the block freezes with its final
    /// success or error background.
    pub(super) fn close_explore_block(&mut self) {
        if let Some(block) = self.explore_block.take() {
            block.borrow_mut().close();
            self.chat_container
                .borrow_mut()
                .mark_changed(&(Rc::clone(&block) as ComponentRef));
            if !block.borrow().is_running() {
                self.chat_container
                    .borrow_mut()
                    .mark_stable(&(block as ComponentRef));
            }
        }
    }

    /// Ends the run on an aborted or failed turn, the reference way
    /// (`pending_explore_tools.drain()` → `complete_call(true)`, then
    /// `close_active_explore()`): every call that never reported back is
    /// failed, calls that already completed keep their result — a block whose
    /// calls all succeeded settles green even on an aborted turn.
    pub(super) fn abort_explore_block(&mut self) {
        for block in &self.chat_explore_blocks {
            block.borrow_mut().fail_running_calls();
            self.chat_container
                .borrow_mut()
                .mark_changed(&(Rc::clone(block) as ComponentRef));
            if !block.borrow().is_open() {
                self.chat_container
                    .borrow_mut()
                    .mark_stable(&(Rc::clone(block) as ComponentRef));
            }
        }
        self.close_explore_block();
    }

    /// The open search block, or a fresh one appended to the chat.
    pub(super) fn open_explore_block(
        &mut self,
        replayed: bool,
    ) -> Rc<RefCell<ExploreBlockComponent>> {
        if let Some(block) = self
            .explore_block
            .as_ref()
            .filter(|block| block.borrow().is_open())
        {
            return Rc::clone(block);
        }
        let block = Rc::new(RefCell::new(ExploreBlockComponent::new()));
        if replayed {
            block.borrow_mut().mark_replayed();
        }
        block.borrow_mut().set_expanded(self.tool_output_expanded);
        self.chat_container
            .borrow_mut()
            .add_child(Rc::clone(&block) as ComponentRef);
        if !replayed {
            self.chat_container
                .borrow_mut()
                .mark_mutable(&(Rc::clone(&block) as ComponentRef));
        }
        self.chat_expandables
            .push(Rc::clone(&block) as Rc<RefCell<dyn Expandable>>);
        self.chat_explore_blocks.push(Rc::clone(&block));
        self.explore_block = Some(Rc::clone(&block));
        block
    }

    pub(super) fn create_tool_component(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        args: serde_json::Value,
    ) -> Rc<RefCell<ToolExecutionComponent>> {
        let settings = self.settings();
        let core = self.ui.clone();
        let notify = self.chat_container.borrow().external_change_sender();
        Rc::new_cyclic(|weak: &std::rc::Weak<RefCell<ToolExecutionComponent>>| {
            let weak = weak.clone();
            RefCell::new(ToolExecutionComponent::new(
                tool_name,
                tool_call_id,
                args,
                ToolExecutionOptions {
                    show_images: Some(settings.get_show_images()),
                    image_width_cells: Some(settings.get_image_width_cells() as usize),
                },
                None,
                Rc::new(move || {
                    if let Some(component) = weak.upgrade() {
                        notify(Rc::as_ptr(&component).cast::<()>() as usize);
                    }
                    core.request_render();
                }),
                self.cwd(),
            ))
        })
    }
}
