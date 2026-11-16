// Illustrative consequences, not additional measurements or verdicts.
// A scenario describes why a rule matters even when both firmwares pass it.
export const EXAMPLES: Record<string, string> = {};
function group(prefix: string, examples: string[]) {
  examples.forEach((text, i) => { EXAMPLES[`${prefix}-${String(i + 1).padStart(2, "0")}`] = text; });
}
function entries(text: string) {
  for (const line of text.trim().split("\n")) {
    const [ids, example] = line.split(" | ");
    for (const id of ids!.split(" ")) EXAMPLES[id] = example!;
  }
}

group("APS-ACK", [
  "A hub could show a command as failed and resend it even though the light already changed.",
  "Unnecessary receipt messages could add traffic when an automation changes many lights together.",
]);
group("APS-DUP", [
  "One press of a toggle button could turn the light on and immediately back off when the message is repeated.",
  "On a noisy network, repeated delivery attempts could make one button press change the light several times.",
  "A light could switch back a few seconds after a button press when a late copy finally arrives.",
]);
group("APS-RETRY", [
  "A brief burst of interference could lose a message that another delivery attempt would have recovered.",
  "A hub could wait longer for a response, making a device interview or an automation feel slow.",
]);
entries(`
APS-COUNTER-01 | A new command could be mistaken for a repeat and ignored, or a repeat could be treated as new.
APS-GROUP-01 | A bedroom bulb could react to a command intended only for the kitchen group.
APS-EP-01 | A hub could miss one of the bulb's functions when sending a command intended for every function.
APS-BIND-01 | A saved connection from a device to one light could send its message to the wrong destination.
APS-BIND-02 | A group binding could reach only one lamp, or cause every lamp to send an unnecessary receipt.
APS-EXT-01 | A hub using the optional longer message format could get no answer to an otherwise ordinary request.
APS-FRAG-01 | A long message could arrive in pieces but never become a complete request the bulb can use.
APS-FRAG-02 | The sender could repeat parts already delivered, or wait for a part the bulb failed to confirm.
GP-EP-01 | A hub might not recognise the bulb as a possible forwarder for a battery-free wall switch. This check covers the advertised service, not every switch interaction.
OTA-NOTIFY-01 | A hub could announce an update while the bulb never asks how to download it.
NWK-CONFLICT-01 | Two devices sharing an address could appear intermittently unreachable or receive traffic meant for the other.
NWK-PANID-01 | After the hub changes the network identifier, some bulbs could remain on the old network and stop answering.
ZDO-CHAN-01 | A bulb left on the old radio channel could disappear from the hub after a channel change.
NWK-UNK-01 | A hub could wait for an answer to a management command instead of learning that the bulb cannot support it.
`);
group("BDB-JOIN", [
  "A bulb could join successfully but remain missing from the hub's device list until another discovery attempt.",
  "A new bulb could fail secure setup or keep using the initial setup key instead of its replacement.",
  "Pairing a second device could cause unnecessary traffic to devices that do not manage joining.",
  "A sensor near a newly paired bulb could be unable to join through it.",
  "A joining invitation could close before you finish putting the next device into pairing mode.",
  "A bulb could waste time trying networks that are not accepting devices.",
  "Pairing could take longer because the bulb checks less likely channels before the preferred ones.",
]);
group("NWK-LEAVE", [
  "After removal in the hub, the bulb could keep trying to use its old network.",
  "A temporary leave-and-return operation could be treated as a permanent removal.",
  "An incorrectly handled network-wide removal message could disconnect bulbs that were never meant to leave.",
]);
group("NWK-REJOIN", [
  "A bulb told to reconnect could remain offline or return using an address the network no longer expects.",
  "A bulb switched off during a key change could stay unreachable when power returns.",
]);
group("ZDO-LEAVE", [
  "The hub could report removal as failed because the bulb leaves before confirming the request.",
  "A removal request sent to many devices could disconnect more bulbs than intended.",
  "Removing a different device could accidentally disconnect this bulb.",
]);
group("NWK-LS", [
  "Other routers could choose poor paths because the bulb stops sharing useful connection reports.",
  "The bulb could choose a neighbour it hears well even though that neighbour cannot hear it in return.",
  "After you switch off a hallway bulb, other devices could keep sending through that missing router and become slow or unreachable.",
]);
group("NWK-BCAST", [
  "A group command could stop spreading before it reaches bulbs farther from the hub.",
  "A moment of interference could make one room miss a command that the rest of the house receives.",
  "One group toggle could run twice on a bulb, leaving it in its original state while other lights change.",
]);
group("NWK-RREQ", [
  "A device beyond direct range of the hub could fail to find a route through this bulb.",
  "The bulb could be reachable by radio but fail to answer a search for a path to it.",
  "Repeated route searches could add unnecessary traffic just when the network is already struggling.",
]);
group("NWK-RREP", ["A route could be found but the reply never get back to the device that needs it."]);
group("NWK-ROUTE", [
  "Messages could stall at this bulb or circulate longer than their allowed travel limit.",
  "A command could reach a device while its reply has no working route back.",
]);
group("NWK-MTO", [
  "The hub could receive a sensor report without learning the path it needs to send a reply.",
  "The bulb could select a path through a router that cannot receive its return traffic.",
]);
group("NWK-SRC", [
  "A command could take the wrong next step even though the hub supplied a complete route.",
  "The hub could keep using a broken chosen route because it never receives the right failure report.",
]);
group("NWK-FAIL", [
  "A command could disappear at this bulb while the sender waits instead of looking for another route.",
  "Switching off a router could leave the sender retrying the same broken path.",
]);
group("NWK-SEC", [
  "A network that expects protected messages could accept an unprotected command.",
  "An old recorded command could change the light again when it should be rejected as a repeat.",
]);
group("PAR-JOIN", [
  "A sensor could connect to the bulb but never complete the hub's security setup.",
  "A sensor pairing through the bulb could fail to receive the key it needs to join.",
]);
group("PAR-POLL", [
  "A battery device could miss a message simply because it was asleep when the message arrived.",
  "Old messages for a device that has gone away could occupy space needed by active devices.",
]);
group("PAR-RELAY", ["A sensor connected through the bulb could send reports that never reach the hub."]);
group("PAR-TMO", [
  "A sleeping sensor and its parent bulb could disagree on how long the sensor may remain silent.",
  "An unsupported timeout could leave a sensor removed too early or retained indefinitely.",
  "A malformed keepalive setup could be accepted even though the bulb and sensor interpret it differently.",
]);
group("PAR-ADDR", [
  "The hub could fail to find the current network address of a sleeping sensor.",
  "The hub could fail to match a sensor's temporary network address to its permanent identity.",
]);
group("PAR-BCAST", ["A sleeping device could miss a message meant for it, or receive traffic intended only for always-awake routers."]);
group("PAR-RREC", ["A sensor report could reach the hub without the route information needed to contact that sensor later."]);
group("PAR-RREQ", ["A sleeping sensor could appear unreachable because its parent does not answer route searches on its behalf."]);
group("PAR-LEAVE", [
  "A permanently removed sensor could remain listed as connected through the bulb.",
  "A sensor briefly reconnecting could be reported to the hub as permanently removed.",
]);
group("PAR-AGE", [
  "A discarded or unpowered sensor could occupy a connection slot needed by a new device.",
  "A sensor whose battery died could be reported as deliberately removed rather than temporarily silent.",
  "A healthy sensor could be disconnected despite sending its regular keepalive messages.",
]);
group("PAR-UNK", ["A sensor that still thinks this bulb is its parent could keep asking for messages without being prompted to reconnect."]);
group("PAR-MOVED", [
  "After a sensor moves to another router, the old parent could keep attracting messages that it can no longer deliver.",
  "The bulb could miss a sensor's move announcement and keep stale information about how to reach it.",
]);
group("PAR-REJOIN", [
  "A known sensor could fail to reconnect after a brief power or radio interruption.",
  "A sensor with an old network key could be unable to recover through this bulb.",
]);
group("PAR-REMOVE", ["A removal instruction from an unauthorised device could disconnect a sensor using the bulb as its parent."]);
group("KEY-TCLK", [
  "A valid key replacement could fail because the bulb mistakes the new message numbering for an old replay.",
  "A bulb could remain stuck in incomplete security setup instead of retrying and leaving cleanly.",
]);
group("KEY-NWK", [
  "After a key change, the bulb could keep sending with the old key and lose contact with the network.",
  "Neighbours could become unreachable immediately after a valid key change because their new message numbers look old.",
  "A bulb could miss a network-wide key update and later stop understanding the hub.",
  "If the switch announcement is lost, a bulb could stay on the old key despite already knowing the new one.",
  "An unauthorised key change could leave a bulb unable to communicate with its real hub.",
]);
group("KEY-APS", ["A previously accepted private command could be carried out again when its encrypted copy is replayed."]);
group("KEY-REMOVE", ["A bulb could leave its network on a removal message that lacks the required protection."]);
group("ZCL-BASIC", [
  "A hub could misidentify the bulb's power source or the command format it supports.",
  "A hub could choose compatibility workarounds for the wrong revision of the command standard.",
  "The device page could lack the version or build date needed to identify which firmware has a reported problem.",
  "The hub could lack the product information used to identify the light or link to its documentation.",
  "Resetting light settings could unexpectedly require re-pairing the bulb or rebuilding its groups.",
]);
group("ZCL-IDENT", [
  "The hub could keep showing the original blink duration rather than how long identification has left.",
  "A request to find one bulb could fail to start blinking, or a stop request could leave it blinking.",
  "A controller trying to find the blinking bulb could receive no useful answer.",
  "A controller could mistake a bulb that is not identifying itself for the one being selected.",
  "A remote's device-finding process could miss the blinking bulb or pick one that is not being identified.",
  "The bulb could keep blinking after the identification period should have ended.",
  "The hub could say identification is active while every bulb looks unchanged.",
  "The standard blink button in a hub could do nothing.",
]);
group("ZCL-COLOR", [
  "A hub could offer colour controls for a bulb that cannot use them, leaving sliders that do nothing.",
  "A hub could lack the white-tone limits or saved power-on tone needed to show reliable controls.",
  "The hub could show a white tone outside the range the lamp says it supports.",
  "Moving the warm/cool slider could change the reported setting without producing the expected light.",
  "An automation using a tone from another bulb model could fail instead of selecting the nearest supported tone.",
  "A sunset automation could jump straight to warm white instead of changing gradually.",
  "The hub could display a transition as still running after the light has stopped changing.",
  "Releasing a warm/cool control could make the bulb jump to warm white instead of staying at the tone you chose.",
  "A stop command could leave an earlier white-tone fade running.",
  "A single warm/cool button press could overshoot the requested step or the lamp's limits.",
  "The hub could report a motionless white-tone command as successful instead of identifying its invalid speed.",
  "A scheduled colour adjustment could silently change tomorrow's switch-on tone while the bulb is off.",
  "An automation that intentionally prepares a tone while the light is off could be ignored, or a one-command exception could be missed.",
  "Lights meant to become cooler as they brighten could end at different tones despite the same configured limit.",
  "A hub could report a hue change as successful on a lamp that only supports warmer and cooler white.",
  "After a power cut, the lamp could return at a different white tone from the one you selected.",
]);
group("ZCL-DIAG", [
  "A hub could interpret the troubleshooting readings using the wrong format.",
  "A diagnostic value could be displayed incorrectly because the hub reads it as a different kind of number.",
  "A weak connection could look healthy if the displayed signal reading belongs to an older message.",
  "A troubleshooting tool could wait for an unsupported command instead of receiving a clear refusal.",
  "A busy bulb could appear idle, hiding the traffic involved in a delivery problem.",
  "Repeated unexpected restarts could go unnoticed if the restart count does not increase.",
]);
group("ZCL-FND-READ", [
  "The hub could fail to display a valid brightness or device-information value.",
  "A missing setting could appear as an empty or misleading value instead of being identified as unsupported.",
  "One unsupported reading could prevent the hub from discovering other readings that the bulb does provide.",
]);
group("ZCL-FND-WRITE", [
  "A setting could be saved while the hub reports failure because the success reply is unreadable.",
  "A value intended only as a reading could be overwritten by a configuration mistake.",
  "A number sent in the wrong format could be stored as a different setting from the one intended.",
  "An automation could believe it configured a setting that does not exist on this bulb.",
  "An invalid value could leave a setting in a state the hub cannot interpret or offer in its controls.",
  "One invalid setting could prevent an unrelated valid setting in the same request from taking effect.",
  "An all-or-nothing configuration change could be only partly applied, leaving settings that disagree with each other.",
  "An unexpected reply could add traffic or confuse a controller that sent a write requiring no reply.",
]);
group("ZCL-FND-DISC", [
  "The hub could miss settings or repeatedly ask for the same ones while building its device page.",
  "The hub could stop discovery too early or keep searching after the last setting.",
]);
group("ZCL-FND-DR", [
  "The light could change correctly while the hub times out waiting for confirmation.",
  "A large automation could generate unnecessary replies from every bulb.",
  "An unsupported command could look like a silent success, leaving an automation unaware that nothing happened.",
  "A network-wide command could provoke a burst of simultaneous replies and more radio collisions.",
  "Two devices could keep replying to each other's replies, adding traffic without doing useful work.",
  "A hub using the current standard could misinterpret an outdated error response.",
  "A command for a vendor-specific feature could unexpectedly turn the light off.",
  "The hub could wait for a response from a feature the bulb does not have.",
]);
group("ZCL-FND-MALF", ["A damaged or incomplete request could make a bulb blink even though no valid blink command was sent."]);
group("ZCL-FND-CLREV", ["The hub could be unable to determine which version of a feature's commands the bulb understands."]);
group("ZCL-FND-REP", [
  "The hub could think automatic reporting setup failed even after the bulb accepted it.",
  "An automation could wait forever for reports from a reading the bulb does not have.",
  "The hub and bulb could disagree on how a reported value is encoded.",
  "Contradictory reporting intervals could be accepted without telling the hub which schedule will actually be used.",
  "The hub could display reporting settings that differ from the bulb's saved configuration.",
  "Changing the light with a remote could leave the hub showing its old state.",
  "A hub could mark a quiet bulb unavailable because its expected periodic reports never arrive.",
  "Rapid dimming could generate more reports than requested, adding radio traffic.",
  "The bulb could keep sending periodic readings even though the hub asked for changes only.",
  "A disabled report could continue to consume airtime.",
  "A state change could be sent to the wrong recipient or go nowhere after a saved connection is removed.",
]);
group("ZCL-GROUP", [
  "A bulb added to the living-room group could be missing when that group is switched on.",
  "An invalid group number could be saved and later produce confusing membership results.",
  "Retrying a group-setup request could make the hub report an error even though the bulb is already in the group.",
  "The hub could confuse a nonexistent group with an invalid request and choose the wrong recovery action.",
  "The hub could show incorrect group membership or think there is room to save more groups when there is not.",
  "Removing a bulb from a group could leave the hub unsure whether removal worked.",
  "A bulb could keep reacting to room commands after all its group memberships were cleared.",
  "A remote intended to select the blinking lamp could add another lamp to its group.",
  "A saved group name could disappear or be reported even though names are not supported.",
  "A lamp outside the kitchen group could respond to the kitchen switch.",
  "A group-management broadcast could make every bulb answer at once and crowd the radio channel.",
  "The hub could believe another group was saved even though the bulb had no space for it.",
  "A hub asking the whole room for group membership could receive no answer from this bulb.",
]);
group("ZCL-SCENE", [
  "The hub could show a scene as active after someone manually changed the light.",
  "A scene could be stored against a group the lamp cannot belong to.",
  "Editing a saved scene could leave the old brightness or transition time in the bulb.",
  "The hub could show settings for a scene that was never saved.",
  "Recalling a reading scene could restore a different light state from the one you saved.",
  "An automation could report that a missing scene was recalled successfully while the light remains unchanged.",
  "A scene could be saved for a group the bulb is not in, making later recall confusing.",
  "A deleted scene could still appear in the bulb or a bulk deletion could remove the wrong entries.",
  "The hub could show an incomplete scene list or incorrect remaining storage.",
  "A scene that changes only colour could unexpectedly dim the lamp to its lowest brightness.",
  "Updating a scene's brightness could erase its name or the slow fade you previously set.",
  "A bedtime scene could jump to dim light instead of taking the requested fade time.",
  "Scenes from a deleted room group could remain stored and take up space.",
  "A hub asking several bulbs which scenes they have could miss this bulb's saved scenes.",
  "A scene intended for one lamp could require an unnecessary group to be created first.",
  "A short scene transition could lose its fractional timing when saved and read back.",
  "Copying a scene to a new group could fail or leave the hub believing a copy exists when it does not.",
]);
group("ZCL-LEVEL", [
  "The hub could lack the readings needed to display brightness and power-on options correctly.",
  "A slow dimming automation could jump immediately to its final brightness.",
  "A command asking for the default fade could use an unexpected duration.",
  "Holding a dimming button could change brightness at the wrong speed or unexpectedly turn the light off.",
  "Releasing a dimming button could leave the earlier fade running.",
  "One press of a brightness button could change the level too far or switch the bulb off unexpectedly.",
  "A fade-up command sent to an off bulb could change only its saved level while the room stays dark.",
  "A bedtime fade could leave a faint glow all night instead of turning the bulb fully off.",
  "Dimming a night light to its minimum could turn it off even though the command was meant to keep it lit.",
  "An ordinary dimming command could change the bulb's saved brightness while it is supposed to ignore changes when off.",
  "An automation preparing a low brightness before switching on could be ignored, causing a bright flash on the next on command.",
  "A command-specific exception could be ignored, leaving the next switch-on level different from what the automation prepared.",
  "Turning a light off and on could lose the previously selected brightness setting.",
  "A lamp configured to switch on at low brightness could return at its previous high level.",
  "The hub could keep showing a fade in progress after the light has settled.",
  "A reserved brightness value could appear as a normal level that other controllers cannot interpret consistently.",
  "Dimming with a remote could leave the app's brightness slider out of date.",
  "After a power cut, the light could come back brighter than its saved power-on setting.",
  "After off and on, the app could show the same brightness while the room is visibly dimmer.",
]);
group("ZCL-ONOFF", [
  "The hub could show the light as off while it remains lit, or a toggle could fail to change its state.",
  "The hub could lack the settings needed for timed switching and power-on behaviour.",
  "An effect-based switch-off could leave an old timer running or lose the state meant to be restored later.",
  "Turning a lamp back on with saved-state recall could lose its earlier brightness.",
  "An old saved state could unexpectedly replace a newer light setting.",
  "An ordinary on command could leave an obsolete saved state available for a later accidental recall.",
  "Fading a light on could leave an older saved state waiting to overwrite the new brightness.",
  "Recalling a scene could leave an older saved state waiting to overwrite that scene.",
  "A hallway light set to turn off automatically could remain on after its timer expires.",
  "A command meant only to extend an already-lit lamp's timer could turn on a dark room.",
  "A motion automation could turn the lamp straight back on during a deliberate waiting period after switch-off.",
  "An ordinary on command could leave an old waiting period interfering with later timed commands.",
  "A timer from before a toggle-off could affect a later switch-on.",
  "A timer could remain active after a fade-to-off and interfere with later light commands.",
  "Fading a bulb on could leave a stale waiting period that blocks the next timed command.",
  "A lamp configured to stay off after a power cut could unexpectedly light up when power returns.",
]);
group("ZDO-NODE", [
  "The hub could fail to recognise that the bulb can forward other devices' messages.",
  "A hub looking for a different device could receive this bulb's description and confuse the two.",
  "The hub could choose behaviour intended for a different Zigbee revision.",
]);
group("ZDO-POWER", ["The hub could display an incorrect or missing power-source description."]);
group("ZDO-ACTEP", ["Device setup could miss some of the bulb's controls because it does not list its functions correctly."]);
group("ZDO-SIMPLE", [
  "The hub could discover the bulb but fail to create its brightness or colour controls.",
  "A malformed discovery request could receive misleading information instead of a clear error.",
  "The hub could wait for a description of a function that the bulb does not use.",
  "A search for another device could incorrectly return this bulb's capabilities.",
]);
group("ZDO-MATCH", [
  "A controller searching for a dimmable light could overlook this bulb.",
  "A direct search for an unsupported feature could time out instead of returning an empty match.",
  "A room-wide capability search could miss matching bulbs or receive replies from irrelevant devices.",
]);
group("ZDO-ADDR", [
  "The hub could know which bulb it wants but fail to learn its current network address.",
  "A lookup for a removed device could return misleading address information.",
  "A network-wide address lookup could receive replies from devices other than the one being sought.",
  "An unsupported lookup format could be accepted without a usable answer.",
  "The hub could associate a temporary address with the wrong permanent device identity.",
  "A controller using the extended address lookup could fail to identify an otherwise working bulb.",
]);
group("ZDO-BIND", [
  "A saved direct connection between devices could appear successful but fail to deliver later commands or reports.",
  "Removing a connection that is already absent could give the hub a misleading result.",
  "A network-wide request could create unintended direct connections on multiple devices.",
  "The hub could be unable to distinguish an empty connection list from a bulb that does not support listing it.",
]);
group("ZDO-LQI", ["The hub's network map could omit the bulb's neighbours, making a coverage problem harder to diagnose."]);
group("ZDO-RTG", ["The hub could be unable to show which paths the bulb knows, even while it forwards traffic."]);
group("ZDO-PERMIT", [
  "Pairing could close too soon, or remain open after the period you selected in the hub.",
  "Opening pairing for the whole network could trigger unnecessary replies from every router.",
]);
group("ZDO-NWKUPD", [
  "The hub could lack the radio-activity readings needed to compare channels before moving the network.",
  "An invalid scan request could leave the hub waiting instead of receiving a clear refusal.",
]);

entries(`
att_write_cmd_not_answered | A phone sending a write that needs no reply could receive an unexpected error or confirmation and lose track of the exchange.
att_unknown_command_ignored | An unsolicited answer to an unknown one-way command could confuse the phone's request handling.
att_signed_write_ignored | A write whose signature cannot be checked could change a value without the required authentication.
att_mandatory_requests | A generic Bluetooth app could connect but fail to discover or read the bulb's basic values.
att_unsupported_request | The app could appear stuck while waiting for a reply to a feature the bulb does not support.
att_read_multiple | Reading several values at once could leave the phone waiting until its request times out.
att_read_multiple_variable | An app using the newer multi-value reading method could hang during setup instead of learning that the method is unsupported.
att_invalid_pdu_short | A request missing part of its address or value type could be acted on as a different request.
att_invalid_pdu_long | An incorrectly formed request could be accepted, hiding a compatibility error in the phone's software.
att_invalid_pdu_uuid_size | The phone and bulb could interpret a malformed value-type identifier differently and return unrelated data.
att_error_rsp_format | The phone could be told that a different value caused an error and troubleshoot the wrong setting.
att_error_codes_permitted | An app could take the wrong recovery action because an error code does not make sense for its request.
att_find_info_invalid_handles | A Bluetooth discovery tool could receive invented entries or the wrong error for an impossible search range.
att_find_info_not_found | The phone could stop or resume discovery at the wrong place because the no-match reply names the wrong starting point.
att_find_info_format | A phone could skip or duplicate controls while discovering what the bulb exposes.
att_find_info_range | A discovery reply could include values outside the part of the bulb the phone is currently inspecting.
att_find_by_type_value_invalid_handles | A malformed search for a particular service could return a plausible match instead of being rejected.
att_find_by_type_value_not_found | The app could wait for a missing service instead of learning that no matching value exists.
att_find_by_type_value_plain_attribute | A discovery tool could list the same value repeatedly or give it an impossible boundary.
att_mtu_exchange_rsp | The phone and bulb could disagree on the largest message either side can receive.
att_mtu_applied | A reply could be larger than the phone accepts, or unnecessarily short after both sides agree to larger messages.
att_mtu_read_by_type_fill | Discovering all controls could take extra requests because each reply leaves room unused.
att_mtu_below_default | A phone proposing an invalid small limit could receive unexpectedly truncated values afterwards.
att_mtu_never_exceeded | A phone using the standard default limit could reject an oversized reply.
att_mtu_server_request | Repeated size negotiations could delay normal reads, or replies could exceed the limit before agreement.
att_mtu_server_request_notifications | A phone could receive a status update before message-size negotiation is complete and handle it inconsistently.
att_read_by_type_values | A phone could display the wrong value or fail to read the beginning of a longer value correctly.
att_read_by_type_uuid_forms | One Bluetooth app could find a control while another cannot, solely because they use different equivalent identifier forms.
att_read_by_type_fill | The phone could need extra reads, or misread a reply that combines values of incompatible sizes.
att_read_by_type_not_found | A phone searching for an absent kind of value could wait instead of getting a clear no-match answer.
att_read_by_type_invalid_handles | An invalid read range could return data or an error referring to the wrong value.
att_read_by_type_not_readable | A phone could receive a protected value or mistake an unreadable value for a missing one.
att_read_invalid_handles | An app could offer to retry with different permissions when the real problem is that the requested value does not exist.
att_read_long_value | A long device description could be cut off or assembled with missing or repeated sections.
att_read_blob_offsets | Reading a long value in parts could skip characters, repeat them or fail at the exact end.
att_read_blob_invalid_offset | An empty reply could make a read beyond the end look valid, hiding an incorrect position in the app.
att_read_blob_invalid_handles | The app could confuse a nonexistent value with a value it merely lacks permission to read.
att_read_by_group_type_fill | A phone could discover only part of the service list or misidentify where a service ends.
att_read_by_group_type_unsupported | A phone could treat an ordinary value as a service containing unrelated controls.
att_read_by_group_type_secondary | Searching for optional secondary services could stall even though the bulb has none.
att_read_by_group_type_errors | A phone could confuse an invalid search with a valid but empty service list.
att_notification_format | A status update could be attributed to the wrong control or discarded because it is too large.
att_unsolicited_confirmation | Answering a stray confirmation could create an exchange with no real request behind it.
att_write_invalid_handles | The app could report a permissions problem when it is actually writing to a nonexistent setting.
att_write_not_permitted | A phone could accidentally alter the descriptions it relies on to understand the bulb's services.
att_prepare_write | Cancelling staged changes could leave the next write reporting an error despite there being nothing left to apply.
att_queued_write_execute | A setting could change before the user confirms it, or several prepared changes could be applied in the wrong order.
att_prepare_write_errors | An invalid prepared change could survive in the queue and interfere with a later valid update.
gap_adv_data_format | A phone could fail to decode the bulb's discovery message and omit it from the device list.
gap_adv_flags | A phone could search for a connection mode the bulb does not actually offer.
gap_limited_discoverable_timeout | The bulb could remain marked as temporarily discoverable long after that temporary period should end.
gap_adv_service_uuids | Repeated service lists could waste the small space available for discovery information or be interpreted differently by phones.
gap_adv_no_gap_gatt_uuid | A generic scanner could list a housekeeping service as an advertised feature. This need not break the vendor app, which expects that entry.
gap_adv_manufacturer_data | A scanner could attribute the bulb's vendor-specific information to the wrong manufacturer.
gap_adv_local_name_once | A phone could show different names for the same bulb depending on which discovery message it reads.
gap_param_update_pause | A phone could be asked to renegotiate timing before its initial setup has settled; whether this causes trouble depends on the phone.
gap_param_update_no_repeat | Repeated timing requests could interrupt setup or add unnecessary traffic to the connection.
gatt_cccd_present | A standard Bluetooth app could have no recognised way to subscribe to the bulb's status updates.
gatt_cccd_default | A newly connected phone could receive updates it never requested because an earlier connection's subscription remained active.
gatt_cccd_write | An app could enable updates but read back a different subscription setting.
gatt_cccd_write_length | A malformed subscription write could change the setting instead of leaving it safely unchanged.
gatt_cccd_bits_follow_properties | An app could believe it enabled confirmed updates even though the bulb does not advertise support for them.
gatt_cccd_gates_notifications | An app could keep receiving status updates after disabling the standard subscription. The vendor app uses its own subscription command, so its normal controls may still work.
gatt_primary_services | A phone could miss a service or combine controls from services whose reported boundaries overlap.
gatt_service_by_uuid | Searching for one service could give a different answer from listing all services, causing app-specific discovery failures.
gatt_char_declaration_value | A generic Bluetooth tool could show incomplete permissions or identifiers when reading a control's description directly.
gatt_char_definition | A phone could follow a control's description to the wrong value or fail to associate it with a service.
gatt_chars_of_service | An app could display controls from the wrong service or miss controls that belong to the one requested.
gatt_descriptors | A phone could fail to find a control's label or subscription setting, or encounter conflicting copies.
gatt_uuid_grouping | A phone could need more discovery exchanges when different identifier sizes are interleaved; this is primarily an efficiency recommendation.
gatt_included_services | An app could wait for a service inclusion query instead of learning that no services are included.
gatt_service_gatt | After an update changes available services, a phone could keep a stale cached control list. This matters only if those services can change.
gatt_gap_service | A generic Bluetooth app could be unable to find the bulb's standard identity information.
gatt_gap_device_name | A phone could show an unreadable or missing device name before login.
gatt_gap_appearance | A phone could display a missing or incorrect device category.
gatt_gap_name_advertised | A bulb found under one name could appear under a different name after connection, making it harder to recognise.
gatt_props_read | A phone could try to read a control advertised as readable and get an unexpected refusal.
gatt_props_write | One app could write successfully while another refuses even to try because the bulb advertises that the operation is unavailable.
gatt_props_write_without_response | A write that the permissions say should have no effect could nevertheless change the bulb's value.
gatt_user_description | A tool could change a control's text label even though the bulb says that label is read-only.
l2cap_recombination | A command could work when sent in one packet but fail when the phone splits it across several packets.
l2cap_recombination_split_header | A valid message could fail merely because the phone divides it at a different point.
l2cap_fragmentation | A short read could work while a longer reply fails to reach the phone intact.
l2cap_length_mismatch | A malformed message could be processed with extra data that the sender and receiver interpret differently.
l2cap_unassigned_cid | Data for an unknown communication channel could be mistaken for a light-control request.
l2cap_sig_reject_unknown_code | A phone trying an unsupported channel command could wait for a reply instead of moving on.
l2cap_sig_reject_disallowed_code | A request intended for classic Bluetooth could leave a Low Energy connection waiting for an answer.
l2cap_sig_credit_based_requests | A phone trying an optional communication channel could stall instead of falling back to a supported method.
l2cap_sig_identifier | A phone could be unable to match a reply to the request it is waiting for.
l2cap_sig_param_update_req_to_peripheral | A request sent in the wrong direction could remain unanswered, delaying the phone's error recovery.
l2cap_sig_unsolicited_response | A stray response could trigger unnecessary replies or interfere with the next real request.
l2cap_sig_mtu_exceeded | An oversized management message could leave the phone waiting instead of learning the supported limit.
l2cap_param_update_request | An impossible combination of connection intervals and timeouts could cause the phone to reject the change or disconnect.
ll_adv_pdu_header | A phone could ignore the bulb's discovery packets because their format is invalid.
ll_adv_adva_stable | A scanner could mistake successive announcements for different bulbs or fail to reconnect to the same bulb.
ll_adv_chsel_feature | The phone could choose a channel-selection method the bulb cannot follow.
ll_adv_one_pdu_per_channel | Extra discovery transmissions could consume radio time without making the bulb easier to find.
ll_adv_pdu_spacing | A phone scanning briefly could miss part of a discovery round and take longer to find the bulb.
ll_adv_interval | Bulbs advertising in lockstep could repeatedly collide, making discovery slower.
ll_adv_no_other_pdu_types | A phone could see unexpected packet types instead of a normal discoverable bulb.
ll_adv_scan_rsp | A phone could find the bulb but fail to retrieve the extra information needed to identify it.
ll_adv_scan_req_other_device | A request intended for another bulb could provoke an unnecessary reply and interfere with discovery.
ll_adv_scan_req_other_addr_type | Two devices with matching address digits in different address categories could be confused during discovery.
ll_adv_connect_ind_other_device | This bulb could stop advertising while trying to establish a connection intended for another device.
ll_adv_connect_ind_other_addr_type | A connection for another address category could make this bulb temporarily disappear from discovery.
ll_adv_connect_ind_invalid | A malformed connection attempt could leave the bulb unavailable until power is cycled.
ll_conn_csa1_all_channels | The phone and bulb could listen on different channels, dropping an otherwise valid connection.
ll_conn_csa1_sparse_map | A connection could fail after the phone excludes noisy channels, even though both devices still have usable channels.
ll_conn_counts_missed_events | One lost packet could throw off the channel sequence and prevent later packets from being heard.
ll_conn_sn_nesn | A packet could be confirmed incorrectly, leaving a message lost or needlessly repeated.
ll_conn_retransmission | A lost receipt could cause the same app command to be processed twice.
ll_conn_lossy_link | Radio interference could leave app commands missing, repeated or applied out of order.
ll_conn_pdu_header | A phone could reject the bulb's data packets despite good radio reception.
ll_conn_central_clock_error | A connection could work with one phone but fail with another whose clock drifts differently.
ll_conn_widening_after_silence | A brief period of interference could become a permanent disconnect because the bulb listens too narrowly afterwards.
ll_conn_more_data | A phone sending several packets together could have to wait for extra connection rounds, slowing a transfer.
ll_conn_first_packet_window | A connection could fail to start with a phone that sends its first packet later within the allowed window.
ll_conn_second_window | Losing the first packet could prevent a connection that should recover at the next scheduled round.
ll_conn_not_established_six_events | A failed connection attempt could keep the bulb out of the discovery list for several seconds before you can try again.
ll_conn_supervision_timeout | A brief radio interruption could disconnect too early, or a dead connection could occupy the bulb for too long.
ll_conn_peripheral_latency | The bulb could sleep through packets it still needs to hear and lose the connection.
ll_ctrl_conn_update | Changing Bluetooth timing could disconnect the app even though the new settings are valid.
ll_ctrl_conn_update_before_instant | A timing change delivered just before its deadline could be applied late and interrupt the connection.
ll_ctrl_conn_update_at_instant | A valid change arriving exactly on time could leave the app connected on screen while commands stop reaching the bulb.
ll_ctrl_conn_update_instant_passed | A connection could continue after an overdue timing instruction instead of resetting the disagreement cleanly.
ll_ctrl_conn_update_supervision | Switching to a shorter timeout could disconnect the app immediately if the timeout is not restarted correctly.
ll_ctrl_channel_map_update | Avoiding a noisy radio channel could unexpectedly break the connection.
ll_ctrl_channel_map_before_instant | A last-moment channel-list update could leave the bulb listening on the old channels.
ll_ctrl_channel_map_at_instant | A valid channel-list update arriving at changeover could desynchronise the phone and bulb.
ll_ctrl_version_exchange | A phone could fail to identify the Bluetooth implementation it is connected to.
ll_ctrl_version_once | Repeated version announcements could add needless traffic or expose compatibility problems in a strict phone implementation.
ll_ctrl_feature_rsp | A phone could try a feature the bulb does not support, or avoid one it does support.
ll_ctrl_feature_used | The two devices could disagree on which connection features they share.
ll_ctrl_unknown_opcode | A phone testing an unfamiliar feature could wait instead of falling back to supported behaviour.
ll_ctrl_ping | A connection check could go unanswered even though the bulb is still reachable.
ll_ctrl_length_req | A phone attempting faster transfers could fail to negotiate packet size or misunderstand the bulb's limit.
ll_ctrl_own_length_req | The bulb could request packet sizes it cannot receive reliably.
ll_ctrl_phy_req | A phone could try a radio mode the bulb cannot use, interrupting communication.
ll_ctrl_conn_param_req | A valid timing proposal could be ignored, or the phone could wait for an unsupported procedure.
ll_ctrl_other_optional | A phone probing optional features could stall instead of receiving a clear refusal.
ll_ctrl_wrong_length | A malformed but recognised control request could leave the phone waiting without an answer.
ll_ctrl_enc_req | The phone could retry encryption setup for the wrong reason instead of recognising missing support or missing keys.
ll_ctrl_terminate | The bulb could keep using a connection after the phone has asked to close it.
ll_ctrl_peripheral_terminate | The phone could continue showing a connection the bulb has already abandoned.
ll_ctrl_peripheral_terminate_timer | A missing disconnect receipt could keep the bulb occupied indefinitely or end the exchange too early.
ll_ctrl_procedure_timeout | An unanswered setup exchange could leave the app appearing connected without making progress.
smp_pairing_request | A phone could keep showing a pairing spinner instead of being told that standard pairing is unsupported.
smp_any_command | A phone could get stuck partway through security setup instead of receiving a clear refusal.
smp_reserved_code | A malformed security message could provoke an unexpected reply and disturb the ongoing setup.
`);
