# Go TUI parity with the ratatui TUI

*2026-10-20 (#516): the ratatui TUI is retired.* The five Rust files below
are deleted; their rows stay as the record of the rules each held, and
`scripts/checks/go-parity.sh` keeps checking that every Go test a row
names exists. A rule marked "not ported" is still not ported. Of the
`tui_run.rs` rows that say "tested in Rust where it lives", the two
`tier_label_vocabulary_` tests and `tier_label_colibri_escapes_an_esc_byte_`
now live in `crates/trellis-cli/src/hosted.rs`, against `ColibriTier::label`
and `runtime_tier_label` directly; the other `tier_label_colibri_` rows
tested the status bar's probe, which went with the file, and the run's
own probe is held by `crates/trellis-cli/tests/colibri_run.rs`.

ADR 0052 section 7: the Go TUI (`tui/`) replaces the ratatui TUI once every
rule the ratatui TUI's tests hold maps to a Go test that passes in
`scripts/checks/go-test.sh` on both runners. This file has one row per test
in the five files the ADR names: `app.rs` (58), `view.rs` (37), `events.rs`
(10), `tui_bridge.rs` (17) and `tui_run.rs` (35), 157 rows. (The ADR counted
23 in `view.rs` and 23 in `tui_run.rs` when it was written; the files have
since grown. Batch 46 added 40 tests, for #634, #635, #636, #639, #640,
#641, #655, #660 and #661; their rows follow each file's earlier ones, each
under the ticket whose area it falls in.) The routing rule is listed after
them.

Each row gives the Rust test, the rule it holds (its name, which in this
repository is written as the rule), the ticket that ports it, and the Go
test once ported, or "not ported" with the reason. A Go test may hold more
than one row.

`scripts/checks/go-parity.sh` fails when a row names a Go test that does
not exist under `tui/`, or when a test in those five files has no row. With
`--complete` it also fails for a row with neither a Go test nor "not
ported" (#514 runs it so). The tickets: T2 #508, T3 #509, T4 #510, T5 #511,
T6 #512, R4 #502 (`list_paths`, whose client side is T5).

| Rust file | Rust test | Rule | Ticket | Go test |
| --- | --- | --- | --- | --- |
| `app.rs` | `esc_on_an_empty_line_quits` | esc on an empty line quits | T3 (#509) | `TestModelEscClearsTheLineThenQuitsWhenIdle` |
| `app.rs` | `quit_is_still_reachable_as_a_message` | quit is still reachable as a message | T2 (#508) | `TestModelCtrlCIdleQuitsAndEndsOnExit` |
| `app.rs` | `a_release_event_changes_nothing` | a release event changes nothing | T2 (#508) | not ported: Bubble Tea delivers key presses only; there is no release or repeat event to filter (crossterm reported both on Windows) |
| `app.rs` | `the_transcript_is_capped` | the transcript is capped | T2 (#508) | `TestModelTranscriptCapHolds` |
| `app.rs` | `a_shell_command_typed_here_does_not_become_an_agent_run` | a shell command typed here does not become an agent run | T5 (#511) | `TestSlashTrellisSubcommandIsNotAShell` |
| `app.rs` | `codebase_is_handed_to_the_driver_rather_than_answered_or_refused` | codebase is handed to the driver rather than answered or refused | T5 (#511) | `TestSlashCodebaseAsksGetCodebase` |
| `app.rs` | `codebase_is_listed_in_help_beside_the_command_it_is_not` | codebase is listed in help beside the command it is not | T5 (#511) | `TestSlashHelpListsTheCommands` |
| `app.rs` | `graph_is_a_command_and_reports_the_reach_without_a_run` | graph is a command and reports the reach without a run | T5 (#511) | `TestSlashGraphAndRadiusReportCountsNeverARatio` |
| `app.rs` | `graph_appears_in_help` | graph appears in help | T5 (#511) | `TestSlashHelpListsTheCommands` |
| `app.rs` | `help_and_model_are_answered_without_a_run` | help and model are answered without a run | T5 (#511) | `TestSlashHelpListsTheCommands`, `TestSlashModelAsksGetState` |
| `app.rs` | `an_unknown_slash_command_is_refused_rather_than_run` | an unknown slash command is refused rather than run | T5 (#511) | `TestSlashUnknownIsRefused` |
| `app.rs` | `clear_empties_the_transcript_and_says_so` | clear empties the transcript and says so | T5 (#511) | `TestSlashClearEmptiesTheTranscript` |
| `app.rs` | `an_ordinary_task_is_still_a_task` | an ordinary task is still a task | T2 (#508) | `TestModelSentTaskBecomesOnePrompt` |
| `app.rs` | `a_reply_starts_its_own_line_and_does_not_land_on_the_task_echo` | a reply starts its own line and does not land on the task echo | T2 (#508) | `TestModelTurnsFillTheTranscript` |
| `app.rs` | `a_second_task_also_gets_its_own_line` | a second task also gets its own line | T2 (#508) | `TestModelRunEndSetsTheOutcome` |
| `app.rs` | `a_multi_line_failure_keeps_every_line_marked` | a multi line failure keeps every line marked | T2 (#508) | `TestModelRunEndErrorMarksEveryLine` |
| `app.rs` | `tokens_join_into_lines_rather_than_one_line_each` | tokens join into lines rather than one line each | T2 (#508) | not ported: token streaming is excluded from parity (#281); rpc sends whole frames, never tokens |
| `app.rs` | `y_accepts_the_modal_and_n_rejects_it` | y accepts the modal and n rejects it | T3 (#509) | `TestApproveYesAndNoSendTheirAnswer` |
| `app.rs` | `while_the_modal_is_up_q_does_not_quit` | while the modal is up q does not quit | T3 (#509) | `TestApproveModalTakesTheKeys` |
| `app.rs` | `transcript_scrolling_back_stops_at_the_end_and_resumes_following` | transcript scrolling back stops at the end and resumes following | T4 (#510) | `TestPanesScrollingBackStopsAtTheEndAndResumesFollowing` |
| `app.rs` | `transcript_outcome_is_held_until_the_next_run_starts` | transcript outcome is held until the next run starts | T2 (#508) | `TestModelRunEndSetsTheOutcome` |
| `app.rs` | `work_events_land_where_they_belong` | work events land where they belong | T4 (#510) | `TestPanesBudgetCarriesTheCeilingTheTurnsFiguresAndElapsed`, `TestPanesRadiusTurnCarriesThreeCounts` |
| `app.rs` | `at_file_offers_matches_case_insensitively_shortest_first` | at file offers matches case insensitively shortest first | R4 (#502) / T5 (#511) | not ported: which paths match is trellis's since R4 (#502): list_paths offers the paths starting with the prefix, shortest first, held by rpc.rs's list_paths tests; the client offers them in the order returned (TestCompleteOneChoiceCompletesSeveralAreListed) |
| `app.rs` | `at_file_offers_nothing_unless_the_last_word_starts_with_at` | at file offers nothing unless the last word starts with at | R4 (#502) / T5 (#511) | `TestCompleteNothingOfferedOrStaleChangesNothing` |
| `app.rs` | `at_file_offers_at_most_ten` | at file offers at most ten | R4 (#502) / T5 (#511) / #844 | `TestCompleteListIsCapped`; list_paths also answers at most 10 (LIST_PATHS_LIMIT in rpc.rs, held by its tests) |
| `app.rs` | `at_file_tab_inserts_the_first_candidate_and_a_space` | at file tab inserts the first candidate and a space | R4 (#502) / T5 (#511) | `TestCompleteOneChoiceCompletesSeveralAreListed` |
| `app.rs` | `at_file_tab_with_no_candidate_changes_nothing` | at file tab with no candidate changes nothing | R4 (#502) / T5 (#511) | `TestCompleteNothingOfferedOrStaleChangesNothing` |
| `app.rs` | `at_file_enter_submits_the_line_as_typed` | at file enter submits the line as typed | R4 (#502) / T5 (#511) | `TestCompleteEnterSubmitsTheLineAsTyped` |
| `app.rs` | `typing_then_enter_submits_the_line` | typing then enter submits the line | T2 (#508) | `TestModelSentTaskBecomesOnePrompt` |
| `app.rs` | `q_types_a_q_now_that_there_is_somewhere_to_type` | q types a q now that there is somewhere to type | T2 (#508) | `TestModelTypingBackspaceAndAnEmptyLine` |
| `app.rs` | `esc_clears_a_line_and_leaves_an_empty_one` | esc clears a line and leaves an empty one | T3 (#509) | `TestModelEscClearsTheLineThenQuitsWhenIdle` |
| `app.rs` | `backspace_deletes_and_stops_at_empty` | backspace deletes and stops at empty | T2 (#508) | `TestModelTypingBackspaceAndAnEmptyLine` |
| `app.rs` | `an_empty_line_submits_nothing` | an empty line submits nothing | T2 (#508) | `TestModelTypingBackspaceAndAnEmptyLine` |
| `app.rs` | `a_line_typed_mid_run_is_queued_as_a_steer_not_started_as_a_second_task` | a line typed mid run is queued as a steer not started as a second task | T3 (#509) | `TestSteerMidRunLineIsASteerNotATask` |
| `app.rs` | `a_slash_command_mid_run_is_refused_as_before_and_is_not_a_steer` | a slash command mid run is refused as before and is not a steer | T3 (#509) | `TestSteerSlashCommandMidRunIsRefused` |
| `app.rs` | `the_modal_takes_y_and_n_before_a_steer_can` | the modal takes y and n before a steer can | T3 (#509) | `TestApproveModalTakesTheKeys` |
| `app.rs` | `a_delivered_steer_is_shown_with_its_turn_once` | a delivered steer is shown with its turn once | T3 (#509) | `TestSteerDeliveredOnceAtItsTurn` |
| `app.rs` | `a_steer_never_marks_a_check_or_an_edit_adr_0004` | a steer never marks a check or an edit (ADR 0004) | T3 (#509) | `TestSteerIsNeverAVerdict` |
| `app.rs` | `steers_are_counted_per_run` | steers are counted per run | T3 (#509) | `TestSteerCountedPerRun` |
| `app.rs` | `the_modal_takes_the_keys_while_it_is_up` | the modal takes the keys while it is up | T3 (#509) | `TestApproveModalTakesTheKeys` |
| `app.rs` | `approval_answer_keeps_edits_as_reported_on_n` | approval answer keeps edits as reported on n | T4 (#510) | `TestPanesAnApprovalAnswerKeepsTheEditsAsReported` |
| `app.rs` | `approval_answer_keeps_edits_as_reported_on_y` | approval answer keeps edits as reported on y | T4 (#510) | `TestPanesAnApprovalAnswerKeepsTheEditsAsReported` |
| `app.rs` | `budget_unreported_elapsed_follows_the_drivers_clock_while_running` | budget unreported elapsed follows the drivers clock while running | T4 (#510) | `TestPanesClockKeepsElapsedCurrentWhileRunning` |
| `app.rs` | `budget_unreported_prompt_size_stays_none_through_a_turn_without_one` | budget unreported prompt size stays none through a turn without one | T4 (#510) | `TestPanesTurnWithoutAPromptSizeLeavesItUnreported` |
| `app.rs` | `approval_modal_scroll_stays_inside_the_diff_and_other_keys_do_nothing` | approval modal scroll stays inside the diff and other keys do nothing | T3 (#509) | `TestApproveModalScrollStaysInsideTheDiff` |
| `app.rs` | `page_keys_page_up_moves_ten_rows_back` | page keys page up moves ten rows back | T4 (#510) | `TestPanesPageUpMovesTenRowsBack` |
| `app.rs` | `page_keys_page_down_moves_ten_rows_forward_and_stops_at_the_end` | page keys page down moves ten rows forward and stops at the end | T4 (#510) | `TestPanesPageDownMovesTenRowsForwardAndStopsAtTheEnd` |
| `app.rs` | `page_keys_home_shows_the_first_line_and_page_up_stops_there` | page keys home shows the first line and page up stops there | T4 (#510) | `TestPanesHomeShowsTheFirstLineAndPageUpStopsThere` |
| `app.rs` | `page_keys_end_follows_the_end_again` | page keys end follows the end again | T4 (#510) | `TestPanesEndFollowsTheEndAgain` |
| `app.rs` | `page_keys_are_listed_in_help` | page keys are listed in help | T5 (#511) | `TestSlashHelpListsThePageKeysAndTheStop` |
| `app.rs` | `paste_lands_in_the_input_and_submits_nothing` | paste lands in the input and submits nothing | T2 (#508) | `TestModelPasteIsOneInputAndSubmitsNothing` |
| `app.rs` | `paste_with_crlf_breaks_is_one_space_per_break` | paste with crlf breaks is one space per break | T2 (#508) | `TestModelPasteIsOneInputAndSubmitsNothing` |
| `app.rs` | `worktree_state_a_second_task_clears_the_four_panes` | worktree state a second task clears the four panes | T5 (#511) | `TestStatusWorktreeASecondTaskClearsTheFourPanes` |
| `app.rs` | `worktree_state_follows_how_the_run_ended` | worktree state follows how the run ended | T5 (#511) | `TestStatusWorktreeFollowsHowTheRunEnded` |
| `app.rs` | `ctrl_c_mid_run_asks_the_run_to_stop_and_leaves_once_it_has_finished` | ctrl c mid run asks the run to stop and leaves once it has finished | T3 (#509) | `TestModelCtrlCMidRunSendsAbortThenQuit`, `TestModelStopMidRunEscOnAnEmptyLineStopsAsCtrlCDoes` |
| `app.rs` | `ctrl_c_mid_run_no_longer_applies_after_run_finished` | ctrl c mid run no longer applies after run finished | T3 (#509) | `TestModelStopMidRunNoLongerAppliesAfterTheRunEnded` |
| `app.rs` | `ctrl_c_mid_run_esc_on_an_empty_line_stops_the_same_way` | ctrl c mid run esc on an empty line stops the same way | T3 (#509) | `TestModelStopMidRunEscOnAnEmptyLineStopsAsCtrlCDoes`, `TestModelEscClearsTheLineThenQuitsWhenIdle` |
| `app.rs` | `ctrl_c_mid_run_withdraws_an_open_question_without_answering_it` | ctrl c mid run withdraws an open question without answering it | T3 (#509) | `TestModelStopMidRunWithdrawsTheQuestionAndShowsNoNewOne` |
| `view.rs` | `transcript_follows_its_end_while_new_lines_arrive` | transcript follows its end while new lines arrive | T4 (#510) | `TestViewTranscriptFollowsScrollsBackAndKeepsTheOutcome` |
| `view.rs` | `transcript_scrolled_back_stops_following_and_says_so` | transcript scrolled back stops following and says so | T4 (#510) | `TestViewTranscriptFollowsScrollsBackAndKeepsTheOutcome` |
| `view.rs` | `transcript_outcome_of_a_finished_run_is_on_screen_without_scrolling` | transcript outcome of a finished run is on screen without scrolling | T4 (#510) | `TestViewTranscriptFollowsScrollsBackAndKeepsTheOutcome` |
| `view.rs` | `the_six_regions_are_on_screen_at_eighty_by_twentyfour` | the six regions are on screen at eighty by twentyfour | T4 (#510) | `TestViewSixRegionsAtEightyByTwentyFour` |
| `view.rs` | `budget_line_rate_excludes_queue_wait` | budget line rate excludes queue wait | T4 (#510) | `TestBudgetLineRateExcludesQueueWait` |
| `view.rs` | `budget_line_unknown_queue_wait_counts_as_zero_and_is_not_drawn` | budget line unknown queue wait counts as zero and is not drawn | T4 (#510) | `TestBudgetLineUnknownQueueWaitIsZeroAndNotDrawn` |
| `view.rs` | `budget_line_without_reported_tokens_draws_no_rate_and_no_wait` | budget line without reported tokens draws no rate and no wait | T4 (#510) | `TestBudgetLineWithoutTokensDrawsNoRateAndNoWait` |
| `view.rs` | `budget_line_with_no_model_time_left_draws_no_rate` | budget line with no model time left draws no rate | T4 (#510) | `TestBudgetLineWithNoModelTimeLeftDrawsNoRate` |
| `view.rs` | `it_lays_out_again_at_a_larger_size` | it lays out again at a larger size | T4 (#510) | `TestViewLaysOutAgainAtALargerSize` |
| `view.rs` | `below_eighty_by_twentyfour_it_says_so_instead_of_laying_out` | below eighty by twentyfour it says so instead of laying out | T4 (#510) | `TestViewTooSmallSaysSoInsteadOfLayingOut` |
| `view.rs` | `the_modal_covers_what_is_under_it` | the modal covers what is under it | T3 (#509) | `TestApproveModalCoversTheScreen` |
| `view.rs` | `the_task_line_shows_what_is_typed_and_how_to_send_it` | the task line shows what is typed and how to send it | T4 (#510) | `TestViewTaskLineShowsWhatIsTypedAndHowToSendIt` |
| `view.rs` | `at_file_candidates_are_drawn_under_the_input_line` | at file candidates are drawn under the input line | T5 (#511) / #842 | `TestCompleteCandidatesAreDrawnUnderTheInput`; in at most the rows the body can spare, the last saying how many more there are (`TestViewManyCompletionsNeverPanic`) |
| `view.rs` | `at_file_candidates_are_not_drawn_while_a_run_holds_the_input` | at file candidates are not drawn while a run holds the input | T5 (#511) | `TestCompleteNothingOfferedOrStaleChangesNothing` |
| `view.rs` | `while_running_the_task_line_shows_the_steer_being_typed` | while running the task line shows the steer being typed | T3 (#509) | `TestSteerLineIsShownWhileRunning` |
| `view.rs` | `every_tool_is_a_node_before_any_of_them_is_called` | every tool is a node before any of them is called | T4 (#510) | `TestViewEveryToolIsANodeBeforeAnyIsCalled` |
| `view.rs` | `a_refused_edit_is_on_the_graph_and_is_not_counted_as_a_write` | a refused edit is on the graph and is not counted as a write | T4 (#510) | `TestViewActionsCountWritesNotRefusals` |
| `view.rs` | `a_run_that_only_looked_says_so_rather_than_leaving_it_blank` | a run that only looked says so rather than leaving it blank | T4 (#510) | `TestViewActionsCountWritesNotRefusals` |
| `view.rs` | `a_tool_this_view_has_never_heard_of_still_reaches_the_graph` | a tool this view has never heard of still reaches the graph | T4 (#510) | `TestViewUnknownToolAndOneFocusPerFigure` |
| `view.rs` | `the_edges_converge_on_one_focus` | the edges converge on one focus | T4 (#510) | `TestViewUnknownToolAndOneFocusPerFigure` |
| `view.rs` | `a_long_target_is_cut_rather_than_wrapped_through_the_edges` | a long target is cut rather than wrapped through the edges | T4 (#510) | `TestViewLongTargetsAreCutAndANarrowPaneDrawsNothing` |
| `view.rs` | `an_escape_sequence_in_a_transcript_never_reaches_the_terminal` | an escape sequence in a transcript never reaches the terminal | T4 (#510) | `TestViewNoEscapeSequenceReachesTheScreen` |
| `view.rs` | `the_graph_survives_a_pane_too_narrow_to_draw_it` | the graph survives a pane too narrow to draw it | T4 (#510) | `TestViewLongTargetsAreCutAndANarrowPaneDrawsNothing` |
| `view.rs` | `tier_status_bar_writes_the_word_once_before_the_label` | tier status bar writes the word once before the label | T5 (#511) | `TestStatusTierWordOnceAndPlain` |
| `view.rs` | `tier_status_bar_with_no_label_says_unknown` | tier status bar with no label says unknown | T5 (#511) | `TestStatusProbingUntilATier` |
| `view.rs` | `worktree_state_is_whole_on_row_0_at_80_by_24_with_a_40_character_spec` | worktree state is whole on row 0 at 80 by 24 with a 40 character spec | T5 (#511) | `TestStatusWorktreeIsWholeAtEightyColumnsWithALongSpec` |
| `view.rs` | `worktree_state_after_a_restored_run_has_no_edits_pending` | worktree state after a restored run has no edits pending | T5 (#511) | `TestStatusWorktreeFollowsHowTheRunEnded` |
| `view.rs` | `budget_unreported_figures_are_named_not_drawn_as_zero` | budget unreported figures are named not drawn as zero | T4 (#510) | `TestBudgetLineUnreportedFiguresAreNamedNotDrawnAsZero`, `TestBudgetLineNamesWhatWasNotReported` |
| `view.rs` | `budget_unreported_elapsed_tick_is_drawn_as_a_clock` | budget unreported elapsed tick is drawn as a clock | T4 (#510) | `TestPanesClockKeepsElapsedCurrentWhileRunning`, `TestBudgetLineUnreportedFiguresAreNamedNotDrawnAsZero` |
| `view.rs` | `approval_modal_scroll_keeps_the_question_drawn_at_80_by_24_with_a_60_line_diff` | approval modal scroll keeps the question drawn at 80 by 24 with a 60 line diff | T3 (#509) | `TestApproveModalKeepsTheQuestionDrawnWhileTheDiffScrolls` |
| `view.rs` | `approval_modal_scroll_page_down_reaches_the_last_line_and_y_still_answers` | approval modal scroll page down reaches the last line and y still answers | T3 (#509) | `TestApproveModalKeepsTheQuestionDrawnWhileTheDiffScrolls` |
| `view.rs` | `approval_modal_scroll_title_says_call_for_a_call_that_is_not_an_edit` | approval modal scroll title says call for a call that is not an edit | T3 (#509) | `TestApproveModalNamesACallThatIsNotAnEdit` |
| `view.rs` | `approval_answer_keeps_edits_row_undecorated_and_the_decline_in_the_transcript` | approval answer keeps edits row undecorated and the decline in the transcript | T4 (#510) | `TestPanesAnApprovalAnswerKeepsTheEditsAsReported` |
| `view.rs` | `fits_80_columns_actions_and_radius_labels_are_whole` | fits 80 columns actions and radius labels are whole | T4 (#510) | `TestViewFitsEightyColumnsActionsAndRadiusLabelsAreWhole` |
| `view.rs` | `fits_80_columns_budget_line_ends_with_a_whole_segment` | fits 80 columns budget line ends with a whole segment | T4 (#510) | `TestBudgetLineAtEightyColumnsEndsWithAWholeSegment` |
| `view.rs` | `fits_80_columns_help_has_no_row_ending_mid_word` | fits 80 columns help has no row ending mid word | T5 (#511) | `TestSlashHelpHasNoRowEndingMidWord` |
| `view.rs` | `wrote_in_text_on_the_edit_node_at_80_columns` | wrote in text on the edit node at 80 columns | T4 (#510) | `TestViewWroteInTextOnTheEditNodeAtEightyColumns` |
| `events.rs` | `quit_ends_the_loop_and_returns_ok` | quit ends the loop and returns ok | T2 (#508) | `TestModelCtrlCIdleQuitsAndEndsOnExit` |
| `events.rs` | `pending_messages_are_coalesced_into_one_draw` | pending messages are coalesced into one draw | T2 (#508) | not ported: Bubble Tea's renderer coalesces updates at its own frame rate; this client has no draw loop of its own to hold to it |
| `events.rs` | `a_modal_answer_is_sent_to_the_asker` | a modal answer is sent to the asker | T3 (#509) | `TestApproveYesAndNoSendTheirAnswer` |
| `events.rs` | `a_steer_typed_mid_run_leaves_on_the_steer_channel_and_not_as_a_task` | a steer typed mid run leaves on the steer channel and not as a task | T3 (#509) | `TestSteerMidRunLineIsASteerNotATask` |
| `events.rs` | `a_steer_is_drawn_queued_then_delivered` | a steer is drawn queued then delivered | T3 (#509) | `TestSteerMidRunLineIsASteerNotATask`, `TestSteerDeliveredOnceAtItsTurn` |
| `events.rs` | `a_steer_before_a_task_is_ahead_of_that_tasks_boundary` | a steer before a task is ahead of that tasks boundary | T3 (#509) | `TestSteerAfterTheRunEndedIsATask`, `TestSteerCountedPerRun` |
| `events.rs` | `paste_event_becomes_one_paste_message` | paste event becomes one paste message | T2 (#508) | `TestModelPasteArrivesAsOneKey` |
| `events.rs` | `ctrl_c_mid_run_emits_one_stop_before_the_loop_ends` | ctrl c mid run emits one stop before the loop ends | T3 (#509) | `TestModelCtrlCMidRunSendsAbortThenQuit`, `TestModelStopMidRunEscOnAnEmptyLineStopsAsCtrlCDoes` |
| `events.rs` | `ctrl_c_mid_run_with_nothing_to_stop_it_leaves_at_once` | ctrl c mid run with nothing to stop it leaves at once | T3 (#509) | `TestModelStopWithNoChildToStopStillEnds` |
| `events.rs` | `ctrl_c_mid_run_draws_the_stopping_line_and_help_says_so` | ctrl c mid run draws the stopping line and help says so | T5 (#511) | `TestSlashHelpListsThePageKeysAndTheStop` |
| `tui_bridge.rs` | `tui_bridge_budget_carries_the_runs_ceiling_and_its_elapsed_time` | tui bridge budget carries the runs ceiling and its elapsed time | T4 (#510) | `TestPanesBudgetCarriesTheCeilingTheTurnsFiguresAndElapsed` |
| `tui_bridge.rs` | `turn_tokens_reach_the_budget` | turn tokens reach the budget | T4 (#510) | `TestPanesBudgetCarriesTheCeilingTheTurnsFiguresAndElapsed` |
| `tui_bridge.rs` | `a_turn_with_a_passing_check_becomes_a_line_and_a_check` | a turn with a passing check becomes a line and a check | T4 (#510) | `TestPanesCheckRowsComeOnlyFromASomeVerdict`, `TestModelTurnsFillTheTranscript` |
| `tui_bridge.rs` | `a_check_kind_written_with_the_display_sigil_names_the_check` | a check kind written with the display sigil names the check | T4 (#510) | not ported: rpc writes every field as JSON before the client reads it (rpc.rs, Fields), so a check_kind arrives as a plain string whichever sigil the loop used; the name reaching the row is TestPanesCheckRowsComeOnlyFromASomeVerdict |
| `tui_bridge.rs` | `a_warning_whose_message_is_a_plain_string_field_still_reaches_the_transcript` | a warning whose message is a plain string field still reaches the transcript | T4 (#510) | `TestPanesAWarningIsALineAndNotATurn`, `TestFramesWarningCarriesItsMessage` |
| `tui_bridge.rs` | `a_failing_check_says_failed_and_is_not_recorded_as_a_pass` | a failing check says failed and is not recorded as a pass | T4 (#510) | `TestPanesCheckRowsComeOnlyFromASomeVerdict` |
| `tui_bridge.rs` | `an_edit_turn_proposes_the_path_it_names` | an edit turn proposes the path it names | T4 (#510) | `TestPanesOnlyAnAppliedWriteIsAPendingEdit` |
| `tui_bridge.rs` | `a_file_write_file_created_is_a_write_and_a_pending_change` | a file write file created is a write and a pending change | T4 (#510) | `TestPanesOnlyAnAppliedWriteIsAPendingEdit` |
| `tui_bridge.rs` | `a_refused_edit_is_not_listed_as_a_pending_change` | a refused edit is not listed as a pending change | T4 (#510) | `TestPanesOnlyAnAppliedWriteIsAPendingEdit` |
| `tui_bridge.rs` | `a_turn_becomes_an_action_whether_or_not_it_wrote` | a turn becomes an action whether or not it wrote | T4 (#510) | `TestPanesOnlyAnAppliedWriteIsAPendingEdit` |
| `tui_bridge.rs` | `an_observation_written_with_the_display_sigil_still_arrives` | an observation written with the display sigil still arrives | T4 (#510) | not ported: rpc writes every field as JSON before the client reads it (rpc.rs, Fields), so there is no sigil for the client to read; the observation reaching the transcript and the edit rule is TestPanesObservationLinesFollowTheTurn |
| `tui_bridge.rs` | `the_no_progress_warning_is_not_counted_as_another_turn` | the no progress warning is not counted as another turn | T4 (#510) | `TestPanesAWarningIsALineAndNotATurn` |
| `tui_bridge.rs` | `a_radius_turn_carries_its_three_counts` | a radius turn carries its three counts | T4 (#510) | `TestPanesRadiusTurnCarriesThreeCounts`, `TestViewRadiusIsThreeCountsNeverARatio` |
| `tui_bridge.rs` | `a_warning_reaches_the_transcript_even_though_it_is_not_a_turn` | a warning reaches the transcript even though it is not a turn | T4 (#510) | `TestPanesAWarningIsALineAndNotATurn` |
| `tui_bridge.rs` | `an_ordinary_debug_line_is_not_mistaken_for_a_turn` | an ordinary debug line is not mistaken for a turn | T4 (#510) | `TestPanesAWarningIsALineAndNotATurn`, `TestFramesWrongTypesAreAbsent` |
| `tui_bridge.rs` | `a_steer_delivery_is_numbered_in_order_and_names_the_turn_it_reached` | a steer delivery is numbered in order and names the turn it reached | T3 (#509) | `TestSteerDeliveredOnceAtItsTurn` |
| `tui_bridge.rs` | `budget_unreported_prompt_size_is_none_not_zero` | budget unreported prompt size is none not zero | T4 (#510) | `TestPanesTurnWithoutAPromptSizeLeavesItUnreported` |
| `tui_run.rs` | `a_model_spec_that_does_not_parse_is_refused_before_the_terminal` | a model spec that does not parse is refused before the terminal | T2 (#508) | `TestModelFailedSetModelIsShown` |
| `tui_run.rs` | `a_runtime_that_cannot_be_reached_does_not_claim_a_tier` | a runtime that cannot be reached does not claim a tier | T5 (#511) | `TestStatusProbingUntilATier` |
| `tui_run.rs` | `a_scripted_model_is_labelled_as_one_without_asking_anything` | a scripted model is labelled as one without asking anything | T5 (#511) | not ported: the tier label is computed by trellis and served as `get_state`'s `tier` (#269); the rule is tested in Rust where it lives |
| `tui_run.rs` | `tier_label_colibri_accepted_says_tools_probed` | tier label colibri accepted says tools probed | T5 (#511) | not ported: the tier label is computed by trellis and served as `get_state`'s `tier` (#269); the rule is tested in Rust where it lives |
| `tui_run.rs` | `tier_label_colibri_refused_says_tier_1_unavailable` | tier label colibri refused says tier 1 unavailable | T5 (#511) | not ported: the tier label is computed by trellis and served as `get_state`'s `tier` (#269); the rule is tested in Rust where it lives |
| `tui_run.rs` | `tier_label_colibri_not_verified_says_tools_assumed` | tier label colibri not verified says tools assumed | T5 (#511) | not ported: the tier label is computed by trellis and served as `get_state`'s `tier` (#269); the rule is tested in Rust where it lives |
| `tui_run.rs` | `tier_label_colibri_not_identified_keeps_native_tool_calls` | tier label colibri not identified keeps native tool calls | T5 (#511) | not ported: the tier label is computed by trellis and served as `get_state`'s `tier` (#269); the rule is tested in Rust where it lives |
| `tui_run.rs` | `tier_label_colibri_escapes_an_esc_byte_in_the_served_id` | tier label colibri escapes an esc byte in the served id | T5 (#511) | not ported: the tier label is computed by trellis and served as `get_state`'s `tier` (#269); the rule is tested in Rust where it lives |
| `tui_run.rs` | `tier_label_colibri_repository_spec_with_key_set_sends_nothing` | tier label colibri repository spec with key set sends nothing | T5 (#511) | not ported: the tier label is computed by trellis and served as `get_state`'s `tier` (#269); the rule is tested in Rust where it lives |
| `tui_run.rs` | `tier_label_vocabulary_every_backend_s_label_has_a_documented_shape` | tier label vocabulary every backend's label has a documented shape | T5 (#511) | not ported: the tier label is computed by trellis and served as `get_state`'s `tier` (#269); the rule is tested in Rust where it lives |
| `tui_run.rs` | `tier_label_vocabulary_shape_check_refuses_what_the_contract_does_not_list` | tier label vocabulary shape check refuses what the contract does not list | T5 (#511) | not ported: the tier label is computed by trellis and served as `get_state`'s `tier` (#269); the rule is tested in Rust where it lives |
| `tui_run.rs` | `the_status_bar_takes_the_tier_the_probe_reports` | the status bar takes the tier the probe reports | T5 (#511) | `TestStatusShowsModelAndTierFromGetState` |
| `tui_run.rs` | `a_line_split_across_writes_arrives_as_one_message` | a line split across writes arrives as one message | T2 (#508) | `TestRPCLineSplitAcrossWritesIsOneLine` |
| `tui_run.rs` | `several_lines_in_one_write_arrive_separately` | several lines in one write arrive separately | T2 (#508) | `TestRPCSeveralLinesAndAnUnterminatedLastOne` |
| `tui_run.rs` | `a_final_line_without_a_newline_is_not_swallowed` | a final line without a newline is not swallowed | T2 (#508) | `TestRPCSeveralLinesAndAnUnterminatedLastOne` |
| `tui_run.rs` | `blank_spacing_lines_do_not_reach_the_pane` | blank spacing lines do not reach the pane | T2 (#508) | `TestModelServedTextIsPlainAndBlankLinesAreDropped` |
| `tui_run.rs` | `a_run_that_fails_before_its_first_turn_still_says_why` | a run that fails before its first turn still says why | T2 (#508) | `TestModelRunEndErrorMarksEveryLine` |
| `tui_run.rs` | `a_multi_line_failure_arrives_as_lines_rather_than_one_run_on` | a multi line failure arrives as lines rather than one run on | T2 (#508) | `TestModelRunEndErrorMarksEveryLine` |
| `tui_run.rs` | `a_failure_that_renders_to_nothing_is_still_not_silence` | a failure that renders to nothing is still not silence | T2 (#508) | `TestModelRunEndErrorMarksEveryLine` |
| `tui_run.rs` | `a_run_that_ends_reports_its_exit_code_rather_than_a_verdict` | a run that ends reports its exit code rather than a verdict | T2 (#508) | `TestModelRunEndSetsTheOutcome` |
| `tui_run.rs` | `a_run_that_panics_reports_it_and_hands_input_back` | a run that panics reports it and hands input back | T2 (#508) | `TestModelChildExitMidRunIsReported` |
| `tui_run.rs` | `every_ending_hands_input_back` | every ending hands input back | T2 (#508) | `TestModelEveryEndingHandsInputBack` |
| `tui_run.rs` | `an_approval_question_becomes_the_modal_and_its_key_goes_back` | an approval question becomes the modal and its key goes back | T3 (#509) | `TestApproveYesAndNoSendTheirAnswer`, `TestApproveQuestionsAreAnsweredOneAtATimeByTheirOwnID` |
| `tui_run.rs` | `an_interface_that_goes_away_mid_question_has_not_said_yes` | an interface that goes away mid question has not said yes | T3 (#509) | `TestApproveLeavingMidQuestionSaysNoYes` |
| `tui_run.rs` | `a_model_the_person_chose_in_the_tui_is_not_refused_as_the_repository_s` | a model the person chose in the tui is not refused as the repository's | T2 (#508) | `TestModelStartupSetsTheModelBeforeAnyPrompt` |
| `tui_run.rs` | `tui_steer_reaches_the_loop_and_the_trail_and_is_reported_before_the_run_ends` | tui steer reaches the loop and the trail and is reported before the run ends | T3 (#509) | `TestSteerAnswersMarkItDeliveredOrNot` |
| `tui_run.rs` | `tui_steer_typed_while_a_run_works_is_forwarded_to_it` | tui steer typed while a run works is forwarded to it | T3 (#509) | `TestSteerMidRunLineIsASteerNotATask` |
| `tui_run.rs` | `tui_steer_for_the_next_task_is_not_forwarded_to_the_run_ending` | tui steer for the next task is not forwarded to the run ending | T3 (#509) | `TestSteerAfterTheRunEndedIsATask` |
| `tui_run.rs` | `tui_steer_left_from_an_ended_task_is_skipped_up_to_the_boundary` | tui steer left from an ended task is skipped up to the boundary | T3 (#509) | `TestSteerCountedPerRun` |
| `tui_run.rs` | `tui_steer_that_arrives_too_late_is_still_in_the_trail` | tui steer that arrives too late is still in the trail | T3 (#509) | not ported: the trail is trellis's to write, and rpc keeps every steer it was sent in it (rpc.rs, SteeringPort), which the client cannot read; the client's half, saying a late steer was not delivered, is TestSteerAnswersMarkItDeliveredOrNot |
| `tui_run.rs` | `budget_unreported_elapsed_ticks_come_from_the_driver_s_clock` | budget unreported elapsed ticks come from the driver s clock | T4 (#510) | `TestPanesClockTicksOnceASecond` |
| `tui_run.rs` | `worktree_state_follows_the_exit_the_run_returned` | worktree state follows the exit the run returned | T5 (#511) | `TestStatusWorktreeFollowsHowTheRunEnded` |
| `tui_run.rs` | `ctrl_c_mid_run_reaches_the_run_which_rolls_back_as_interrupted` | ctrl c mid run reaches the run which rolls back as interrupted | T3 (#509) | not ported: the stop reaches the run as rpc's `abort`, which trellis carries out and rolls back (rpc.rs, abort_interrupts_the_run_and_it_rolls_back); the client's half, sending abort once before quit, is TestModelCtrlCMidRunSendsAbortThenQuit |
| `tui_run.rs` | `ctrl_c_mid_run_drops_an_open_question_only_once_the_run_has_stopped` | ctrl c mid run drops an open question only once the run has stopped | T3 (#509) | not ported: rpc's abort_run drops the open questions only once the run has observed the abort (rpc.rs), the order this test holds for the Rust driver; the client's half, withdrawing the question unanswered, is TestModelStopMidRunWithdrawsTheQuestionAndShowsNoNewOne |
| `tui_run.rs` | `ctrl_c_mid_run_with_a_question_open_still_ends_and_rolls_back` | ctrl c mid run with a question open still ends and rolls back | T3 (#509) | not ported: a client that stops with a question open sends abort and closes rpc's input, and trellis declines the question and exits (rpc.rs, end_of_input_with_a_question_open_declines_it_and_the_process_exits); the client's half is TestApproveLeavingMidQuestionSaysNoYes |

## Routing

The rule that a typed line is a question or a task is trellis's, asked
through rpc `classify` (ADR 0052 decision 2.3, R1 #501). The Rust surfaces
are held to it by `crates/trellis-cli/tests/hygiene.rs`.

| Rust file | Rust test | Rule | Ticket | Go test |
| --- | --- | --- | --- | --- |
| `hygiene.rs` | `every_conversational_surface_asks_whether_the_input_is_a_task` | every conversational surface asks whether the input is a task | T6 (#512) | `TestRouteAQuestionSendsNoPrompt`, `TestRouteATaskSendsOnePrompt`, `TestRouteAFailedClassifyRunsNothing` |
