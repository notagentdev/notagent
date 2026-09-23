use super::*;

impl InteractiveMode {
    // ------------------------------------------------------------------
    // Transcript
    // ------------------------------------------------------------------

    pub(super) fn render_initial_messages(&mut self) {
        let entries = self.session().with_session_manager(|manager| {
            manager
                .get_branch(None)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        });
        self.render_session_entries(&entries, true);
        self.render_project_trust_warning_if_needed();

        let compaction_count = self
            .session()
            .with_session_manager(|manager| manager.get_entries())
            .iter()
            .filter(|entry| matches!(entry, SessionEntry::Compaction { .. }))
            .count();
        if compaction_count > 0 {
            let times = if compaction_count == 1 {
                "1 time".to_owned()
            } else {
                format!("{compaction_count} times")
            };
            self.show_status(&format!("Session compacted {times}"));
        }
    }

    pub(super) fn render_session_entries(
        &mut self,
        entries: &[SessionEntry],
        populate_history: bool,
    ) {
        let mut items: Vec<AgentMessage> = Vec::new();
        for entry in entries {
            if let Some(record) = TaskLifecycleRecord::from_session_entry(entry) {
                items.push(AgentMessage::Custom(CustomMessage {
                    custom_type: TASK_LIFECYCLE_ENTRY_TYPE.to_owned(),
                    content: UserContent::Text(String::new()),
                    display: true,
                    details: serde_json::to_value(&record).ok(),
                    timestamp: record.task.base().started_at,
                }));
                continue;
            }
            items.extend(session_entry_to_context_messages(entry));
        }
        self.render_session_items(&items, populate_history);
    }

    /// The cache-miss notices and the custom session entries belong to the
    /// slices that own them.
    pub(super) fn render_session_items(&mut self, items: &[AgentMessage], populate_history: bool) {
        self.pending_tools.clear();
        self.pending_tool_by_call.clear();
        self.settled_tool_calls.clear();
        self.chat_tool_rows.clear();
        self.chat_explore_blocks.clear();
        self.explore_block_by_call.clear();
        self.explore_block = None;
        self.chat_expandables.clear();
        let mut rendered_pending: Vec<(String, Rc<RefCell<ToolExecutionComponent>>)> = Vec::new();

        for item in items {
            match item {
                AgentMessage::Assistant(message) => {
                    self.add_message_to_chat(item, populate_history);
                    // Reproduce the live text/thinking boundary before this
                    // restored message's calls open their own block.
                    if streamed_visible_chars(message) > 0 {
                        self.close_explore_block();
                    }
                    for content in message.content.iter() {
                        let notagent_ai::types::AssistantContent::ToolCall(call) = content else {
                            continue;
                        };
                        // No transcript row for `todo_write` or `task` on
                        // restore either — same deviation as
                        // `add_tool_component`; subagents replay through their
                        // lifecycle rows.
                        if call.name == "todo_write" || call.name == "task" {
                            continue;
                        }
                        let args = serde_json::Value::Object(call.arguments.clone());
                        if is_background_bash_call(&call.name, &args) {
                            continue;
                        }
                        // Restored searches group like live ones; the block is
                        // replayed history and closes at the end of the items.
                        if is_explore_tool(&call.name) {
                            let block = self.open_explore_block(true);
                            block.borrow_mut().push_call(
                                &call.name,
                                call.id.clone(),
                                &serde_json::Value::Object(call.arguments.clone()),
                            );
                            self.explore_block_by_call
                                .insert(call.id.clone(), Rc::clone(&block));
                            if matches!(
                                message.stop_reason,
                                StopReason::Aborted | StopReason::Error
                            ) {
                                block.borrow_mut().complete_call(&call.id, true);
                            }
                            continue;
                        }
                        self.close_explore_block();
                        let component = self.create_tool_component(&call.name, &call.id, args);
                        component
                            .borrow_mut()
                            .set_expanded(self.tool_output_expanded);
                        self.chat_container
                            .borrow_mut()
                            .add_child(Rc::clone(&component) as ComponentRef);
                        self.chat_tool_rows.push(Rc::clone(&component));
                        self.chat_expandables
                            .push(Rc::clone(&component) as Rc<RefCell<dyn Expandable>>);

                        if matches!(message.stop_reason, StopReason::Aborted | StopReason::Error) {
                            let error_message = if message.stop_reason == StopReason::Aborted {
                                let attempt = self.session().retry_attempt();
                                if attempt > 0 {
                                    format!(
                                        "Aborted after {attempt} retry attempt{}",
                                        if attempt > 1 { "s" } else { "" }
                                    )
                                } else {
                                    "Operation aborted".to_owned()
                                }
                            } else {
                                message
                                    .error_message
                                    .clone()
                                    .unwrap_or_else(|| "Error".to_owned())
                            };
                            component
                                .borrow_mut()
                                .update_result(error_result(&error_message), false);
                        } else {
                            rendered_pending.push((call.id.clone(), component));
                        }
                    }
                    // After the message's own calls have joined the block, so
                    // that a restored abort settles the same single block the
                    // live path settles — not a fresh one behind it.
                    if matches!(message.stop_reason, StopReason::Aborted | StopReason::Error) {
                        self.abort_explore_block();
                    }
                }
                AgentMessage::ToolResult(result) => {
                    if let Some(index) = rendered_pending
                        .iter()
                        .position(|(id, _)| id == &result.tool_call_id)
                    {
                        let (_, component) = rendered_pending.remove(index);
                        component
                            .borrow_mut()
                            .update_result(result_from_message(result), false);
                    } else {
                        // Search results have no row; route them to the block
                        // carrying the call.
                        if let Some(block) = self.explore_block_by_call.get(&result.tool_call_id) {
                            block
                                .borrow_mut()
                                .complete_call(&result.tool_call_id, result.is_error);
                        }
                    }
                }
                message => self.add_message_to_chat(message, populate_history),
            }
        }

        // Restored history is over; whatever block is still open freezes.
        self.close_explore_block();
        self.pending_tools.extend(rendered_pending);
        self.pending_tool_by_call.extend(
            self.pending_tools
                .iter()
                .map(|(id, component)| (id.clone(), Rc::clone(component))),
        );
        if self.cell.mode() == TuiMode::Regular {
            self.chat_container
                .borrow_mut()
                .replay_recent(self.ui.columns());
        }
        self.cell
            .request_render(self.cell.mode() == TuiMode::Regular);
    }

    pub(super) fn add_message_to_chat(&mut self, message: &AgentMessage, populate_history: bool) {
        // A new message row between searches ends the block (the streamed
        // assistant path closes it at MessageEnd instead).
        // A tool result belongs to a call the block already carries, so it
        // must not end the run — closing here gave every call its own block.
        // Everything else that lands in the transcript does end it, matching
        // the events the reference closes on (user message, compaction,
        // interrupt).
        if !matches!(
            message,
            AgentMessage::Assistant(_) | AgentMessage::ToolResult(_)
        ) {
            self.close_explore_block();
        }
        match message {
            AgentMessage::User(message) => {
                let text = user_message_text(message);
                if text.is_empty() {
                    return;
                }
                if !self.chat_container.borrow().children.is_empty() {
                    self.chat_container
                        .borrow_mut()
                        .add_child(component_ref(Spacer::new(1)));
                }
                match parse_skill_block(&text) {
                    Some(block) => {
                        let component =
                            Rc::new(RefCell::new(SkillInvocationMessageComponent::new(
                                block.clone(),
                                Some(get_markdown_theme()),
                            )));
                        component
                            .borrow_mut()
                            .set_expanded(self.tool_output_expanded);
                        self.chat_container
                            .borrow_mut()
                            .add_child(component as ComponentRef);
                        if let Some(user_message) = block.user_message {
                            let mut chat = self.chat_container.borrow_mut();
                            chat.add_child(component_ref(Spacer::new(1)));
                            chat.add_child(component_ref(UserMessageComponent::new(
                                user_message,
                                Some(get_markdown_theme()),
                                Some(self.output_pad),
                                Vec::new(),
                            )));
                        }
                    }
                    None => {
                        self.chat_container.borrow_mut().add_child(component_ref(
                            UserMessageComponent::new(
                                text.clone(),
                                Some(get_markdown_theme()),
                                Some(self.output_pad),
                                Vec::new(),
                            ),
                        ));
                    }
                }
                if populate_history {
                    self.editor.borrow_mut().editor_mut().add_to_history(&text);
                }
            }
            AgentMessage::Assistant(message) => {
                let component = Rc::new(RefCell::new(AssistantMessageComponent::new(
                    Some(message.clone()),
                    self.hide_thinking_block,
                    Some(get_markdown_theme()),
                    Some(self.hidden_thinking_label.clone()),
                    Some(self.output_pad),
                    Vec::new(),
                )));
                component
                    .borrow_mut()
                    .set_expanded(self.tool_output_expanded);
                self.chat_container
                    .borrow_mut()
                    .add_child(Rc::clone(&component) as ComponentRef);
                self.chat_expandables
                    .push(component as Rc<RefCell<dyn Expandable>>);
            }
            AgentMessage::CompactionSummary(message) => {
                let component = Rc::new(RefCell::new(CompactionSummaryMessageComponent::new(
                    message.clone(),
                    Some(get_markdown_theme()),
                )));
                component
                    .borrow_mut()
                    .set_expanded(self.tool_output_expanded);
                let mut chat = self.chat_container.borrow_mut();
                chat.add_child(component_ref(Spacer::new(1)));
                chat.add_child(component as ComponentRef);
            }
            AgentMessage::BranchSummary(message) => {
                let component = Rc::new(RefCell::new(BranchSummaryMessageComponent::new(
                    message.clone(),
                    Some(get_markdown_theme()),
                )));
                component
                    .borrow_mut()
                    .set_expanded(self.tool_output_expanded);
                let mut chat = self.chat_container.borrow_mut();
                chat.add_child(component_ref(Spacer::new(1)));
                chat.add_child(component as ComponentRef);
            }
            AgentMessage::Custom(message) => {
                if message.custom_type == TASK_LIFECYCLE_ENTRY_TYPE {
                    if let Some(record) = message
                        .details
                        .clone()
                        .and_then(|details| serde_json::from_value(details).ok())
                    {
                        self.append_task_lifecycle(&record);
                    }
                } else if message.display {
                    let component = Rc::new(RefCell::new(CustomMessageComponent::new(
                        message.clone(),
                        Some(get_markdown_theme()),
                        Some(self.output_pad),
                    )));
                    component
                        .borrow_mut()
                        .set_expanded(self.tool_output_expanded);
                    self.chat_container
                        .borrow_mut()
                        .add_child(component as ComponentRef);
                }
            }
            // `bashExecution` belongs to the bash slice; tool results are drawn
            // inline with their call.
            AgentMessage::BashExecution(_) | AgentMessage::ToolResult(_) => {}
        }
    }
}
