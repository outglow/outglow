// Reader-facing explanations are separate from generated measurements so a
// fresh comparison run does not replace prose with protocol names or dumps.
import type { Result, Side, Test } from "./compare.ts";

export const CHECKS: Record<string, string> = {};
function group(prefix: string, descriptions: string[]) {
  descriptions.forEach((text, i) => { CHECKS[`${prefix}-${String(i + 1).padStart(2, "0")}`] = text; });
}
function entries(text: string) {
  for (const line of text.trim().split("\n")) {
    const [id, ...words] = line.split(" | ");
    CHECKS[id!] = words.join(" | ");
  }
}

group("APS-ACK", [
  "Confirms receipt when the sender asks for confirmation, identifying the message it received.",
  "Carries out a message without sending a receipt when the sender did not request one.",
]);
group("APS-DUP", [
  "Recognises a command repeated shortly afterwards: confirms receipt again but carries it out only once.",
  "Recognises repeated commands throughout the allowed retry period, even when receipt confirmations are lost.",
  "Recognises a late repeat even when the earlier repeats were lost, so the command runs only once.",
]);
group("APS-RETRY", [
  "Retries a message the expected number of times when no receipt arrives, then gives up.",
  "Waits the required time for a receipt before retrying a message.",
]);
entries(`
APS-COUNTER-01 | Gives each new message the next number, so receivers can distinguish new messages from repeats.
APS-GROUP-01 | Carries out a group command only when the bulb belongs to that group.
APS-EP-01 | Delivers a command addressed to all of the bulb's functions to each function, and ignores a command for a function it does not have.
APS-BIND-01 | Sends a message to the device chosen in a saved direct connection between devices, called a binding.
APS-BIND-02 | Sends a message to the group chosen in a saved binding, without asking every group member for a receipt.
APS-EXT-01 | Accepts an ordinary message even when it uses the optional longer message heading.
APS-FRAG-01 | Puts a long message back together from its parts and tells the sender which parts arrived.
APS-FRAG-02 | Correctly identifies which part of a long message it is confirming receipt of.
GP-EP-01 | Advertises the standard forwarding service for battery-free Zigbee switches, called Green Power.
OTA-NOTIFY-01 | Asks the hub for update details when the hub announces that new firmware is available.
NWK-CONFLICT-01 | Notices when another device uses its network address, reports the conflict and announces a replacement address.
NWK-PANID-01 | Passes a network identifier change to its neighbours before switching, so they can still hear the announcement.
ZDO-CHAN-01 | Moves to the radio channel requested by the hub.
NWK-UNK-01 | Tells the sender when it does not understand a network management command.
`);
group("BDB-JOIN", [
  "Announces its address and abilities to the network after joining.",
  "After joining, obtains and checks a new private encryption key for communication with the hub's security manager.",
  "After joining, asks the hub and routers to let more devices join for the required period.",
  "After joining, offers to accept new devices through itself for the required period.",
  "Keeps its joining invitation open long enough for another device to use it.",
  "Tries to join only networks that are accepting new devices.",
  "Searches the preferred Zigbee radio channels before trying the remaining channels.",
]);
group("NWK-LEAVE", [
  "Announces its departure and stops using the network when directly told to leave without returning.",
  "Announces that it intends to return when told to leave and rejoin.",
  "Ignores a network-wide command telling devices to leave.",
]);
group("NWK-REJOIN", [
  "Can rejoin after being told to leave and return, using the address assigned on its return.",
  "Can recover after missing an encryption key change by obtaining the current key securely from the hub.",
]);
group("ZDO-LEAVE", [
  "Answers a direct request to remove itself before announcing its departure and leaving.",
  "Ignores a device-removal request sent to the whole network.",
  "Rejects a removal request for an unrelated device without leaving the network itself.",
]);
group("NWK-LS", [
  "Regularly tells nearby routers how well it can hear them, using the expected message format.",
  "Updates its view of a connection using what the neighbouring router says it can hear.",
  "Stops treating a neighbouring router as reachable after its regular reports have been missing for too long.",
]);
group("NWK-BCAST", [
  "Passes a network-wide message on after a short random delay, while respecting its remaining travel limit.",
  "Repeats a network-wide message when nearby routers have not been heard passing it on.",
  "Carries out a network-wide command only once and does not forward it beyond its travel limit.",
]);
group("NWK-RREQ", [
  "Passes on a search for a route, adding the difficulty of the connection it arrived over and retrying as required.",
  "Answers a search for a route to itself, including the difficulty of the return connection.",
  "When searching for a route itself, uses the required number and spacing of retries.",
]);
group("NWK-RREP", ["Passes a route-search reply back towards the requester and updates the route's difficulty."]);
group("NWK-ROUTE", [
  "Forwards a message along a known route without letting it travel beyond its limit.",
  "Remembers the return route when forwarding a route-search reply, so communication can work both ways.",
]);
group("NWK-MTO", [
  "Responds to the hub's network-wide route search and reports its route before sending data back.",
  "Ignores a route search received through a neighbour it cannot reach in return.",
]);
group("NWK-SRC", [
  "Forwards a message along the exact sequence of routers chosen by the sender.",
  "Tells the sender when delivery along its chosen sequence of routers fails, identifying the final destination.",
]);
group("NWK-FAIL", [
  "Tells the sender when it cannot forward a message because it has no route to the destination.",
  "Tells the sender when the next router on a known route stops confirming receipt.",
]);
group("NWK-SEC", [
  "Rejects an unprotected network message after joining a secured network.",
  "Rejects an old encrypted message sent again, instead of treating it as a new command.",
]);
group("PAR-JOIN", [
  "Tells the hub's security manager when a new device joins through the bulb.",
  "Passes the hub's encryption-key delivery message to a device that is still joining.",
]);
group("PAR-POLL", [
  "Keeps messages for a sleeping battery device and hands them over when that device wakes and asks.",
  "Discards a message that has waited too long for a sleeping device to collect it.",
]);
group("PAR-RELAY", ["Forwards a battery device's message to its destination, protecting the forwarded message with encryption."]);
group("PAR-TMO", [
  "Accepts a valid request from a battery device to set how long it may stay silent, and explains how to keep the connection alive.",
  "Rejects an unsupported silence timeout requested by a battery device.",
  "Rejects a timeout request that uses reserved configuration settings.",
]);
group("PAR-ADDR", [
  "Answers a request for the network address of a battery device that uses this bulb as its router.",
  "Answers a request for the permanent device address of a battery device that uses this bulb as its router.",
]);
group("PAR-BCAST", ["Passes network-wide messages to sleeping devices only when those messages are meant to reach them too."]);
group("PAR-RREC", ["Reports the route from a battery device to the hub before forwarding that device's data."]);
group("PAR-RREQ", ["Answers route searches on behalf of battery devices connected through the bulb."]);
group("PAR-LEAVE", [
  "Tells the hub when a connected battery device announces that it is leaving permanently.",
  "Does not report a permanent departure when a connected battery device says it will rejoin.",
]);
group("PAR-AGE", [
  "Forgets a battery device that stays silent beyond its agreed timeout.",
  "Forgets a timed-out device without claiming that the device announced a permanent departure.",
  "Keeps a battery device connected while it continues to send the agreed keepalive messages.",
]);
group("PAR-UNK", ["Asks an unrecognised battery device to rejoin when it tries to collect messages from the bulb."]);
group("PAR-MOVED", [
  "Stops claiming a battery device as its own after that device's messages start arriving through another router.",
  "Stops claiming a battery device as its own when another router forwards that device's new announcement.",
]);
group("PAR-REJOIN", [
  "Lets a known battery device rejoin using its existing encryption key and informs the hub.",
  "Helps a returning battery device obtain the current network key through the hub's security manager.",
]);
group("PAR-REMOVE", ["Removes a connected battery device only when instructed by the hub's security manager."]);
group("KEY-TCLK", [
  "Accepts the hub's confirmation using a newly exchanged private key, even when the new message numbering starts over.",
  "Retries an unanswered private-key request and leaves the network if secure setup cannot be completed.",
]);
group("KEY-NWK", [
  "Stores a replacement network encryption key from the hub and uses it when told to switch.",
  "Accepts neighbours' messages numbered from the beginning again after changing the encryption key.",
  "Accepts a protected network-wide key update from the hub and switches keys when instructed.",
  "Switches to a newer key it already knows when it receives a valid message using that key.",
  "Rejects a replacement network key and a key-switch command from a router that is not the network's security manager.",
]);
group("KEY-APS", ["Rejects a repeated old message protected by a private key shared with another device."]);
group("KEY-REMOVE", ["Leaves on the security manager's properly encrypted removal command, and rejects an unprotected one."]);
group("ZCL-BASIC", [
  "Reports the required basic information about its command standard and power source.",
  "Reports the version of the Zigbee command standard being checked here.",
  "Reports its manufacturer, model, hardware and software versions, and build date in the expected format.",
  "Reports its product category, product code and product web address.",
  "Restores default settings when asked, while keeping network membership and saved groups.",
]);
group("ZCL-IDENT", [
  "Counts down the remaining time for making the bulb identify itself visibly.",
  "Starts identifying itself for the requested time, or stops when asked.",
  "While identifying itself, tells a requester how much time remains.",
  "Stays silent when asked about an identification sequence that is not running.",
  "Answers a network-wide identification query only while identifying itself, so a controller can find it.",
  "Stops identifying itself when the countdown finishes.",
  "Visibly changes its light while identifying itself and becomes steady again afterwards.",
  "Accepts the standard request to blink.",
]);
group("ZCL-COLOR", [
  "Provides the required information about supported colour controls whenever it advertises colour support.",
  "Reports the current white tone, its supported range, its power-on setting and the time remaining in a change.",
  "Keeps its reported white tone and brightness-linked limits within the bulb's supported range.",
  "Changes to the requested warmer or cooler white tone and reports that mode correctly.",
  "Uses the nearest supported white tone when asked for one beyond its physical range.",
  "Changes white tone gradually over the requested time.",
  "Reports the time remaining during a white-tone change and no remaining time once finished.",
  "Changes white tone at the requested speed, respects the limits and stops at the current tone when told to stop.",
  "Stops an ongoing white-tone transition when the separate stop command arrives.",
  "Adjusts white tone by the requested step without going beyond the allowed limits.",
  "Rejects a continuous white-tone change with no movement speed, leaving the light unchanged.",
  "Ignores white-tone commands while off when changes while off are disabled.",
  "Allows or blocks white-tone changes while off according to the saved setting or the individual command's override.",
  "Makes dimming change white tone only when that feature is enabled, reaching the configured tone at full brightness.",
  "Reports that an unsupported colour command cannot be carried out.",
  "Uses the chosen white tone after power returns, or restores the previous one if configured to do so.",
]);
group("ZCL-DIAG", [
  "Reports the diagnostics format version expected by the standard being checked.",
  "Returns diagnostic counters using the standard names and value formats understood by a hub.",
  "Reports the signal strength and reception quality of the request just received, reflecting a weaker test signal.",
  "Rejects commands that the standard diagnostics service does not support.",
  "Increases its received-message counters when new messages arrive.",
  "Increases its restart counter when the bulb restarts.",
]);
group("ZCL-FND-READ", [
  "Returns a requested setting or reading with the information the hub needs to interpret it.",
  "Clearly reports that a requested setting or reading does not exist.",
  "Answers each part of a request for several readings, even if one of them does not exist.",
]);
group("ZCL-FND-WRITE", [
  "Saves valid settings and confirms success in the expected reply format.",
  "Refuses to change a value that can only be read.",
  "Refuses a setting written in the wrong value format and keeps the old value.",
  "Reports that a setting does not exist when someone tries to change it.",
  "Rejects a setting outside its allowed range and keeps the previous value.",
  "Applies valid settings in a mixed request and reports only the settings that failed.",
  "For an all-or-nothing settings request, changes everything only if every setting is valid.",
  "Processes settings sent without a reply request and stays silent, even if some are invalid.",
]);
group("ZCL-FND-DISC", [
  "Lists supported settings in order, starting where the hub asks and respecting the requested list length.",
  "Correctly tells the hub whether more settings remain to be discovered.",
]);
group("ZCL-FND-DR", [
  "Confirms a successful command when the sender expects a general reply.",
  "Carries out a successful command silently when the sender asks for no general reply.",
  "Reports an unsupported command even when ordinary success replies are disabled.",
  "Does not send general replies to network-wide commands, avoiding a reply from every bulb.",
  "Does not reply to a reply, avoiding an endless exchange.",
  "Uses the current standard error response for an unsupported general command.",
  "Rejects a command meant for an unknown manufacturer instead of treating it as a normal light command.",
  "Tells the sender when a command addresses a feature the bulb does not have.",
]);
group("ZCL-FND-MALF", ["Rejects an incomplete command without carrying it out."]);
group("ZCL-FND-CLREV", ["Reports a format version for every advertised Zigbee feature."]);
group("ZCL-FND-REP", [
  "Confirms a valid request to configure automatic reports in the expected format.",
  "Rejects a request to report a setting or reading that does not exist.",
  "Rejects a report configuration that gives the wrong value format.",
  "Rejects contradictory reporting intervals, such as a maximum wait shorter than the minimum wait.",
  "Returns the saved reporting configuration, or explains that the requested reading does not exist.",
  "Sends a changed value to the configured recipient when automatic reporting is enabled.",
  "Sends a periodic report before the maximum wait expires, even if the value has not changed.",
  "Waits for the minimum reporting interval before sending another report, even if the value changes again.",
  "Reports changes without periodic repeats when periodic reporting is disabled.",
  "Sends no reports for a reading when its reporting is fully disabled.",
  "Sends reports only to the destinations saved for that feature.",
]);
group("ZCL-GROUP", [
  "Joins a requested group, confirms success and includes it in later membership queries.",
  "Rejects an invalid group number without joining it.",
  "Treats joining a group it already belongs to as success, without creating a duplicate membership.",
  "Distinguishes a group it has not joined from an invalid group number when queried.",
  "Lists matching group memberships and reports how much room remains for more groups.",
  "Removes a group membership and distinguishes missing groups from invalid group numbers.",
  "Removes every group membership when asked.",
  "Joins a group on an identification-based request only while the bulb is identifying itself.",
  "Returns saved group names if it supports them, or clearly indicates that it does not.",
  "Acts on a group command only if it belongs to the group, without sending a general reply.",
  "Processes group-management commands sent to many devices without sending unwanted replies.",
  "Reports that storage is full when another group cannot be added.",
  "Answers a network-wide membership query when it asks about all groups or a group the bulb belongs to.",
]);
group("ZCL-SCENE", [
  "Reports its saved scenes and whether the light still matches the selected scene.",
  "Rejects a scene for an invalid group or a group the bulb has not joined.",
  "Saves a scene exactly as supplied, reports it back and replaces it when the same scene is saved again.",
  "Reports a missing scene or an unjoined group without pretending to return saved settings.",
  "Saves and restores the current light state, and tracks whether later changes still match that scene.",
  "Applies a saved scene, or reports a missing scene without changing the light.",
  "Refuses to save the current light state into a group the bulb has not joined.",
  "Deletes one scene or all scenes in a group and reports missing entries correctly.",
  "Lists a group's scenes and remaining storage, or explains that the bulb has not joined that group.",
  "Restores only the settings actually included in a scene, leaving other settings unchanged.",
  "Keeps a scene's name and transition time when replacing its saved light state.",
  "Takes the requested time to recall a scene, using the scene's saved transition time when no override is given.",
  "Deletes the associated scenes when all group memberships are removed.",
  "Answers a network-wide scene query only when it has a scene for that group.",
  "Allows a scene to be saved and recalled independently of any group.",
  "Supports saving and reading scenes with transition times more precise than whole seconds.",
  "Copies a scene to another scene entry and confirms which scene was copied.",
]);
group("ZCL-LEVEL", [
  "Reports brightness, remaining fade time, dimming options and the power-on brightness setting.",
  "Fades smoothly to the requested brightness over the requested time.",
  "Uses its configured default fade duration when asked to use the default.",
  "Brightens or dims at the requested speed, stopping at its limits without changing the on/off state.",
  "Stops a fade at its current brightness and reports that no fade time remains.",
  "Changes brightness by a requested step within its limits, without changing the on/off state.",
  "Turns an off light on before starting a dimming command that explicitly allows switching it on.",
  "Switches fully off at the end of a fade-to-off command.",
  "Stays on at minimum brightness when the dimming command explicitly leaves switching alone.",
  "Ignores ordinary dimming commands while off when changes while off are disabled.",
  "Updates the saved brightness while keeping the light off when a command explicitly permits it.",
  "Honours both the saved permission to change brightness while off and a command that overrides it.",
  "Remembers the reported brightness across off and on when no fixed switch-on brightness is set.",
  "Uses the configured switch-on brightness when the light is turned on.",
  "Counts down the remaining fade time and reports no remaining time once finished.",
  "Keeps brightness within the allowed range even when a command asks for a reserved value.",
  "Reports brightness changes to the configured recipient.",
  "Uses the selected brightness after power returns, including restoring the previous level when requested.",
  "Restores the LED drive signal in the emulator, as well as the reported brightness, after switching off and on.",
]);
group("ZCL-ONOFF", [
  "Switches on, switches off and toggles correctly, with the reported state matching the light.",
  "Provides the required switching state, timers, saved-state controls and power-on setting.",
  "Switches off with the requested effect, preparing the saved light state for later recall and clearing the on timer.",
  "Switches on and restores the light settings saved by the earlier effect-based switch-off command.",
  "Ignores a saved-state recall when that saved state is no longer available for recall.",
  "Clears the pending saved-state recall when an ordinary on command arrives.",
  "Clears the pending saved-state recall when a dimming command turns the light on.",
  "Clears the pending saved-state recall when a scene turns the light on.",
  "Turns on for the requested duration, counts down and switches off when the time expires.",
  "Honours a timed-on command restricted to an already-lit bulb only while the light is on.",
  "After an early switch-off, blocks another timed-on command until the required waiting period ends.",
  "An ordinary on command clears the waiting period when no on timer is running.",
  "Toggling a lit bulb off clears its on timer.",
  "Dimming the bulb off clears its on timer.",
  "Dimming the bulb on clears a waiting period when no on timer is running.",
  "Uses the selected on/off behaviour after power returns: off, on, previous state or its opposite.",
]);
group("ZDO-NODE", [
  "Describes itself to the hub as a router on the expected Zigbee radio band.",
  "Reports that an unrelated device cannot be found instead of describing itself in that device's place.",
  "Reports which revision of the Zigbee network standard it follows.",
]);
group("ZDO-POWER", ["Returns its power-supply description when the hub asks."]);
group("ZDO-ACTEP", ["Lists its available functions in the expected order so the hub can discover them."]);
group("ZDO-SIMPLE", [
  "Describes the capabilities of a requested function that the bulb provides.",
  "Rejects a function number outside the allowed range.",
  "Reports that a valid but unused function number is inactive.",
  "Reports that an unrelated device cannot be found when asked for its capabilities.",
]);
group("ZDO-MATCH", [
  "Lists its functions that match the capabilities requested by the hub.",
  "Returns an empty list when a direct capabilities query finds no match.",
  "Answers a network-wide capabilities query only if it has a matching function.",
]);
group("ZDO-ADDR", [
  "Returns its current network address when asked using its permanent device address.",
  "Reports that an unknown permanent device address cannot be found.",
  "Answers a network-wide address search only when it is the device being sought.",
  "Rejects an address search that uses an unsupported request format.",
  "Returns the permanent address for its network address and reports unknown addresses correctly.",
  "Returns its addresses when asked using the extended address-query format.",
]);
group("ZDO-BIND", [
  "Saves a direct connection between devices, called a binding, and includes it when asked to list saved connections.",
  "Reports that a saved direct connection does not exist when asked to remove it.",
  "Ignores a binding request addressed to the whole network.",
  "Answers a query for an empty list of saved direct connections.",
]);
group("ZDO-LQI", ["Returns its list of nearby devices, including the hub it joined through, for the hub's network map."]);
group("ZDO-RTG", ["Returns its known routes."]);
group("ZDO-PERMIT", [
  "Opens joining for the requested duration and closes it afterwards, or closes it immediately when asked.",
  "Acts on a network-wide request to open or close joining without sending a reply.",
]);
group("ZDO-NWKUPD", [
  "Measures radio activity on the requested channels and reports the readings to the hub.",
  "Rejects a radio-scan request with an unsupported duration.",
]);

entries(`
att_write_cmd_not_answered | Never replies to a Bluetooth write sent in the form that requires no reply, and keeps protected values unchanged.
att_unknown_command_ignored | Silently ignores unknown Bluetooth commands whose form does not allow a reply.
att_signed_write_ignored | Ignores a signed write it cannot authenticate, without changing the value.
att_mandatory_requests | Supports the basic Bluetooth requests to discover available values and read them.
att_unsupported_request | Tells the phone when a Bluetooth request is unsupported instead of leaving it waiting.
att_read_multiple | Answers a request to read several values together, or explicitly says that this request is unsupported.
att_read_multiple_variable | Answers a request to read several values with their lengths, or explicitly says that this request is unsupported.
att_invalid_pdu_short | Rejects a Bluetooth request with required information missing.
att_invalid_pdu_long | Rejects a fixed-size Bluetooth request with unexpected extra data.
att_invalid_pdu_uuid_size | Rejects a request whose value-type identifier has an invalid length.
att_error_rsp_format | Identifies the failed request and the full number of the affected value in an error reply.
att_error_codes_permitted | Uses an error response that is allowed for the particular request being answered.
att_find_info_invalid_handles | Rejects a discovery request with an invalid starting point or reversed search range.
att_find_info_not_found | Reports no values found and identifies the requested search range correctly.
att_find_info_format | Lists Bluetooth values in order with complete identifiers, fitting the agreed reply size.
att_find_info_range | Lists only values inside the requested search range, starting with the first match.
att_find_by_type_value_invalid_handles | Rejects an invalid search range when looking for values of a particular type and content.
att_find_by_type_value_not_found | Clearly reports when a search by value type and content has no match.
att_find_by_type_value_plain_attribute | Lists every matching individual value once, without pretending it is a group of values.
att_mtu_exchange_rsp | Answers a request to agree a maximum Bluetooth message size with a valid, consistent size limit.
att_mtu_applied | Uses the smaller of the phone's and bulb's message-size limits after they exchange them.
att_mtu_read_by_type_fill | Uses the newly agreed message size to fit as many same-sized readings as allowed in one reply.
att_mtu_below_default | Keeps the standard minimum message size when the phone proposes a smaller, invalid limit.
att_mtu_never_exceeded | Keeps Bluetooth replies within the default size until a larger size is agreed.
att_mtu_server_request | If it starts message-size negotiation, asks only once and uses the default limit until the phone answers.
att_mtu_server_request_notifications | Waits for the phone's answer to its message-size request before sending unsolicited value updates.
att_read_by_type_values | Returns the first matching Bluetooth value with its identifier and the portion that fits in the reply.
att_read_by_type_uuid_forms | Finds the same values whether their type is expressed using the equivalent short or long identifier.
att_read_by_type_fill | Fills a reply with as many same-sized values of the requested type as fit, in the expected order.
att_read_by_type_not_found | Reports when no value of the requested type exists in the search range.
att_read_by_type_invalid_handles | Rejects an invalid search range when reading values by type.
att_read_by_type_not_readable | Reports that reading is forbidden when the first matching value cannot be read.
att_read_invalid_handles | Reports an invalid value number when asked to read a Bluetooth value that does not exist.
att_read_long_value | Lets the phone read a long value in successive parts that fit the agreed message size.
att_read_blob_offsets | Returns the requested part of a value, with an empty reply when reading starts exactly at its end.
att_read_blob_invalid_offset | Rejects a request to start reading beyond the end of a value.
att_read_blob_invalid_handles | Reports an invalid value number when asked to read part of a nonexistent value.
att_read_by_group_type_fill | Lists complete Bluetooth service descriptions in order, filling the reply within the agreed size.
att_read_by_group_type_unsupported | Rejects a service-list request that names a kind of value that is not a service grouping.
att_read_by_group_type_secondary | Clearly reports when no secondary Bluetooth services are available.
att_read_by_group_type_errors | Distinguishes an invalid service search range from a valid range containing no services.
att_notification_format | Identifies the updated value and respects the agreed message size when sending an unsolicited update.
att_unsolicited_confirmation | Ignores a confirmation that does not correspond to an update it sent.
att_write_invalid_handles | Reports an invalid value number when asked to write a Bluetooth value that does not exist.
att_write_not_permitted | Refuses changes to read-only Bluetooth service descriptions and keeps their values unchanged.
att_prepare_write | Stages changes without applying them until confirmed, supports cancellation, or explicitly refuses staged writes if unsupported.
att_queued_write_execute | Applies staged Bluetooth changes only when confirmed, in the order they were prepared.
att_prepare_write_errors | Rejects staged changes to nonexistent or read-only values without keeping those changes for later.
gap_adv_data_format | Formats its Bluetooth discovery information so a phone can separate and read each item.
gap_adv_flags | Correctly describes how it can be discovered and that it uses Bluetooth Low Energy.
gap_limited_discoverable_timeout | Stops claiming to be temporarily discoverable after the allowed discovery period ends.
gap_adv_service_uuids | Does not repeat lists of the same kind of service identifier in one discovery message.
gap_adv_no_gap_gatt_uuid | Leaves basic Bluetooth housekeeping services out of the advertised list of application services, as recommended.
gap_adv_manufacturer_data | Identifies the manufacturer at the start of manufacturer-specific discovery information.
gap_adv_local_name_once | Gives a consistent device name in discovery messages and says whether the name is complete or shortened.
gap_param_update_pause | Waits the recommended settling time before asking the phone to change connection timing.
gap_param_update_no_repeat | Waits after a connection-timing reply before asking for another change.
gatt_cccd_present | Provides the standard subscription setting for each Bluetooth value that can send updates.
gatt_cccd_default | Starts a new, unpaired connection with standard update subscriptions switched off.
gatt_cccd_write | Lets the phone change a standard update subscription and read the chosen setting back.
gatt_cccd_write_length | Rejects an oversized subscription setting without changing it, and handles shorter writes as allowed.
gatt_cccd_bits_follow_properties | Accepts only subscription modes that the corresponding Bluetooth value supports.
gatt_cccd_gates_notifications | Sends value updates only while the phone has enabled them through the standard subscription setting.
gatt_primary_services | Lists all main Bluetooth services once, in order, with clear, non-overlapping boundaries.
gatt_service_by_uuid | Finds a Bluetooth service by its type and returns the same boundaries as a full service listing.
gatt_char_declaration_value | Gives the same complete description of a Bluetooth value whether the phone reads it directly or discovers it in a list.
gatt_char_definition | Describes each Bluetooth value within its service and points to the correct value and type.
gatt_chars_of_service | Lists exactly the Bluetooth values belonging to the requested service.
gatt_descriptors | Lists each value's supporting information, such as labels and subscription settings, without invalid duplicates.
gatt_uuid_grouping | Groups short and long Bluetooth identifiers as recommended, making discovery more predictable.
gatt_included_services | Lists services included inside another service, or explicitly says there are none.
gatt_service_gatt | Provides the required way to notify phones if the list of Bluetooth services can change during the device's lifetime.
gatt_gap_service | Provides one basic Bluetooth identity service with the device name and device category.
gatt_gap_device_name | Lets a phone read a valid text device name before establishing a secure connection.
gatt_gap_appearance | Lets a phone read the standard device-category value before establishing a secure connection.
gatt_gap_name_advertised | Uses the same device name during discovery and after connection, allowing a shortened discovery name.
gatt_props_read | Allows reading only where the Bluetooth value's advertised permissions say it is readable.
gatt_props_write | Accepts or refuses a write request according to the value's advertised write permission.
gatt_props_write_without_response | Keeps a value unchanged if it does not permit writes that require no reply.
gatt_user_description | Provides readable text labels and allows changing them only when that permission is advertised.
l2cap_recombination | Puts a Bluetooth message back together when it arrives in several radio packets.
l2cap_recombination_split_header | Reassembles a message even when its opening information is split across radio packets.
l2cap_fragmentation | Splits a long outgoing Bluetooth message into correctly marked parts.
l2cap_length_mismatch | Discards a Bluetooth message whose actual length does not match its declared length, then processes the next message normally.
l2cap_unassigned_cid | Ignores a Bluetooth message addressed to an unassigned communication channel.
l2cap_sig_reject_unknown_code | Explicitly rejects an unknown Bluetooth channel-management command.
l2cap_sig_reject_disallowed_code | Rejects a channel-management command that belongs to classic Bluetooth rather than Bluetooth Low Energy.
l2cap_sig_credit_based_requests | Answers requests for optional Bluetooth communication channels, explicitly refusing them if unsupported.
l2cap_sig_identifier | Matches each channel-management reply to the request number chosen by the phone.
l2cap_sig_param_update_req_to_peripheral | Rejects a connection-timing request that is being sent in the wrong direction for the bulb's role.
l2cap_sig_unsolicited_response | Silently ignores a channel-management reply that nobody requested.
l2cap_sig_mtu_exceeded | Answers an oversized channel-management message with a size error, or processes it if it fits its supported limit.
l2cap_param_update_request | Uses valid, mutually compatible timing settings when asking the phone to change a connection.
ll_adv_pdu_header | Uses the expected Bluetooth discovery packet format without setting reserved options.
ll_adv_adva_stable | Uses a valid device address consistently across Bluetooth discovery messages.
ll_adv_chsel_feature | Advertises the newer radio-channel selection method only if it actually supports it.
ll_adv_one_pdu_per_channel | Sends at most one discovery message on each radio channel during a discovery round.
ll_adv_pdu_spacing | Keeps discovery messages within one round close enough together for phones to follow them.
ll_adv_interval | Repeats discovery rounds at an allowed interval, adding a small random delay to reduce collisions.
ll_adv_no_other_pdu_types | Sends only the expected discovery message type when no phone has requested anything else.
ll_adv_scan_rsp | Answers a phone's request for more discovery information on the same radio channel and with its own address.
ll_adv_scan_req_other_device | Ignores discovery-information requests addressed to another device.
ll_adv_scan_req_other_addr_type | Ignores a discovery request with matching address digits but a different address category, which means it is for another device.
ll_adv_connect_ind_other_device | Does not start a connection requested for another device.
ll_adv_connect_ind_other_addr_type | Does not connect when the address digits match but the address category identifies a different device.
ll_adv_connect_ind_invalid | Remains discoverable and accepts a later valid connection after receiving invalid connection settings.
ll_conn_csa1_all_channels | Follows the agreed radio-channel hopping sequence when all data channels are available.
ll_conn_csa1_sparse_map | Correctly substitutes an allowed radio channel when the hopping sequence reaches a disabled channel.
ll_conn_counts_missed_events | Keeps the connection schedule and channel sequence moving even when no packet arrives.
ll_conn_sn_nesn | Starts message numbering correctly, confirms new packets and stops resending packets once confirmed.
ll_conn_retransmission | Resends an unconfirmed packet unchanged and processes a repeated incoming packet only once.
ll_conn_lossy_link | Keeps messages complete, in order and free of duplicates when packets are lost in both directions.
ll_conn_pdu_header | Uses valid connected-packet formats and keeps within the currently allowed packet size.
ll_conn_central_clock_error | Keeps hearing the phone even when its clock runs fast or slow within the declared tolerance.
ll_conn_widening_after_silence | Listens for longer after missed packets to allow for growing clock differences and hear the phone again.
ll_conn_more_data | Keeps listening within a connection round when the phone says it has more data to send.
ll_conn_first_packet_window | Hears the phone's first packet anywhere within the agreed opening time window.
ll_conn_second_window | Listens at the next scheduled time and channel if the phone's first packet never arrives.
ll_conn_not_established_six_events | Gives up an attempted connection within the required short deadline if the phone never sends a packet.
ll_conn_supervision_timeout | Keeps an established connection during a brief silence and ends it when the agreed timeout expires.
ll_conn_peripheral_latency | Skips listening rounds only when the connection rules allow it, staying within the timeout.
ll_ctrl_conn_update | Follows a change to the connection schedule and listens throughout the new opening time window.
ll_ctrl_conn_update_before_instant | Applies a timing change at the agreed moment when the instruction arrives just beforehand.
ll_ctrl_conn_update_at_instant | Applies a timing change that arrives exactly when it takes effect, keeping the connection working.
ll_ctrl_conn_update_instant_passed | Ends the connection when a timing-change instruction arrives after its scheduled changeover.
ll_ctrl_conn_update_supervision | Restarts the silence timeout at a timing change so the new settings do not cause a premature disconnect.
ll_ctrl_channel_map_update | Starts using the new set of allowed radio channels at the agreed moment without restarting the hopping sequence.
ll_ctrl_channel_map_before_instant | Applies a channel-list change on time when the instruction arrives just beforehand.
ll_ctrl_channel_map_at_instant | Applies a channel-list change received exactly at its changeover moment.
ll_ctrl_version_exchange | Answers the phone's version exchange with a correctly formatted firmware and manufacturer identification.
ll_ctrl_version_once | Sends its Bluetooth version information only once in a connection.
ll_ctrl_feature_rsp | Replies with its supported Bluetooth features in the expected format.
ll_ctrl_feature_used | Reports the relevant connection features shared by both the phone and the bulb.
ll_ctrl_unknown_opcode | Explicitly reports that an unknown Bluetooth connection-control request is unsupported.
ll_ctrl_ping | Answers a Bluetooth connection check, or explicitly says that this kind of check is unsupported.
ll_ctrl_length_req | Answers a request to use longer radio packets according to whether it supports that feature.
ll_ctrl_own_length_req | Requests longer radio packets only if it supports them, using valid limits.
ll_ctrl_phy_req | Answers a request to change radio transmission mode, or explicitly refuses it if only the basic mode is supported.
ll_ctrl_conn_param_req | Answers a connection-timing proposal according to the features it advertises.
ll_ctrl_other_optional | Explicitly refuses optional connection-control requests for features it does not advertise.
ll_ctrl_wrong_length | Answers a recognised connection-control request even when its length is wrong, as allowed by the standard.
ll_ctrl_enc_req | Refuses an encryption request with the reason appropriate to its advertised support and available keys.
ll_ctrl_terminate | Confirms the phone's disconnect request and stops using the connection.
ll_ctrl_peripheral_terminate | Announces a disconnect it initiates itself and ends the connection once the phone confirms receipt.
ll_ctrl_peripheral_terminate_timer | Retries its own disconnect announcement until confirmed or until the allowed timeout expires.
ll_ctrl_procedure_timeout | Ends a connection when a connection-control request it initiated remains unanswered for too long.
smp_pairing_request | Answers a Bluetooth pairing request, explicitly refusing pairing if unsupported.
smp_any_command | Explicitly refuses Bluetooth security-setup commands when pairing is unsupported.
smp_reserved_code | Silently ignores Bluetooth security-setup messages with reserved command numbers.
`);

// These explanations describe the observed non-passing behaviour, not just
// the inverse of the expectation. Keys include verdict and side so a new
// result cannot accidentally inherit an old failure explanation.
const OBSERVATIONS: Record<string, string> = {};
function observed(side: Side | "both", verdict: string, ids: string, text: string) {
  for (const id of ids.split(" ")) for (const s of side === "both" ? ["stock", "outglow"] : [side]) {
    OBSERVATIONS[`${id}:${s}:${verdict}`] = text;
  }
}
observed("stock", "FAIL", "APS-DUP-03 NWK-BCAST-03", "Carries out a late repeat again. In this test, a toggle command switches the light twice.");
observed("stock", "FAIL", "APS-RETRY-01", "Retries fewer times than required before giving up.");
observed("stock", "FAIL", "APS-RETRY-02", "Waits longer than required before retrying.");
observed("outglow", "N/A", "APS-RETRY-01 APS-RETRY-02", "Sends its replies without asking for a receipt, so it has no retries to measure.");
observed("stock", "FAIL", "APS-EXT-01", "Does not answer the request when it uses the optional longer heading.");
observed("stock", "N/A", "APS-FRAG-01 APS-FRAG-02", "Does not support reassembling a message from parts, and does not claim to support it.");
observed("stock", "FAIL", "BDB-JOIN-03", "Sends the invitation to every device instead of just the hub and routers.");
observed("stock", "FAIL", "BDB-JOIN-07", "Searches every channel in order instead of trying the preferred channels first.");
observed("stock", "N/A", "OTA-NOTIFY-01", "Has no support for receiving firmware updates through Zigbee.");
observed("stock", "FAIL", "NWK-CONFLICT-01", "Does not report that another device is using its network address.");
observed("stock", "FAIL", "NWK-PANID-01", "Switches too soon and forwards the announcement using the new network identifier. Neighbours still using the old identifier cannot hear it.");
observed("stock", "FAIL", "NWK-BCAST-02", "Forwards the message once and does not retry when neighbours fail to pass it on.");
observed("stock", "FAIL", "NWK-RREQ-03", "Sends more route-search attempts than the standard allows.");
observed("stock", "FAIL", "NWK-FAIL-01", "Sends no failure report when it has no route for another device's message.");
observed("stock", "FAIL", "NWK-UNK-01", "Leaves the unknown network command unanswered.");
observed("stock", "FAIL", "NWK-LS-03", "Still reports a silent neighbour as reachable well after the allowed timeout.");
observed("stock", "FAIL", "PAR-MOVED-01", "Keeps claiming the battery device as its own after its messages start arriving through another router.");
observed("stock", "FAIL", "ZCL-BASIC-02", "Reports an older revision of the Zigbee command standard than the one these tests check.");
observed("stock", "FAIL", "ZCL-BASIC-03", "Does not provide the required build-date information.");
observed("stock", "FAIL", "ZCL-BASIC-04", "Does not provide the required product web address.");
observed("stock", "FAIL", "ZCL-IDENT-01", "Leaves the reported identification time unchanged instead of counting down.");
observed("stock", "FAIL", "ZCL-COLOR-01", "Advertises colour controls on the dimmable-only bulb but omits the required information about them.");
observed("stock", "FAIL", "ZCL-COLOR-08", "Jumps to the warmest white when told to stop, instead of stopping at the current tone.");
observed("stock", "FAIL", "ZCL-COLOR-11 ZCL-COLOR-15", "Reports success for a colour command it should reject.");
observed("stock", "FAIL", "ZCL-COLOR-14", "Changes white tone with brightness but misses the configured tone at maximum brightness.");
observed("stock", "FAIL", "ZCL-FND-WRITE-05", "Accepts and stores a value outside the setting's allowed range.");
observed("stock", "FAIL", "ZCL-FND-WRITE-08", "Replies even though this form of write must remain unanswered.");
observed("stock", "FAIL", "ZCL-FND-DISC-02", "Returns settings out of order and repeats an entry, making the discovery list inconsistent.");
observed("stock", "FAIL", "ZCL-FND-DR-06", "Uses an error response retired by the version of the standard being checked.");
observed("stock", "FAIL", "ZCL-FND-DR-07", "Treats the unknown manufacturer's command as an ordinary off command and switches the light off.");
observed("stock", "FAIL", "ZCL-FND-DR-08", "Does not answer a command for a feature the bulb does not have.");
observed("stock", "FAIL", "ZCL-FND-MALF-01", "Carries out an incomplete identification command instead of rejecting it.");
observed("stock", "FAIL", "ZCL-FND-CLREV-01", "Does not provide the required format version for its advertised colour controls.");
observed("stock", "FAIL", "ZCL-FND-REP-01", "Accepts the reporting configuration but formats the success reply incorrectly.");
observed("stock", "FAIL", "ZCL-FND-REP-04", "Accepts a reporting schedule whose maximum wait is shorter than its minimum wait.");
observed("stock", "FAIL", "ZCL-GROUP-02", "Accepts an invalid group number.");
observed("stock", "FAIL", "ZCL-GROUP-03", "Reports a duplicate as an error instead of accepting the existing membership as success.");
observed("stock", "FAIL", "ZCL-GROUP-04 ZCL-GROUP-06 ZCL-SCENE-02", "Rejects the request but gives the wrong reason for an invalid group number.");
observed("stock", "FAIL", "ZCL-GROUP-13 ZCL-SCENE-14", "Does not answer the network-wide membership query.");
observed("stock", "FAIL", "ZCL-SCENE-06", "Reports success when asked to recall a scene that does not exist.");
observed("stock", "FAIL", "ZCL-SCENE-10", "Dims to its lowest level when recalling a scene with no brightness setting, instead of preserving brightness.");
observed("stock", "FAIL", "ZCL-SCENE-12", "Reaches the scene's brightness too soon instead of taking the requested transition time.");
observed("stock", "FAIL", "ZCL-SCENE-13", "Keeps the saved scenes after their groups have been removed.");
observed("stock", "FAIL", "ZCL-LEVEL-08", "Stays on at its dimmest glow instead of switching fully off.");
observed("stock", "FAIL", "ZCL-LEVEL-19", "Reports the same brightness after off and on, but restores a lower LED drive signal in the emulator.");
observed("stock", "N/A", "ZCL-LEVEL-14", "Does not offer a fixed switch-on brightness setting.");
observed("both", "N/A", "ZCL-LEVEL-03", "Does not offer the optional default fade time for switching on and off, so this check does not apply. It still reaches the requested brightness.");
observed("stock", "FAIL", "ZCL-LEVEL-16", "Accepts a reserved brightness value as if it were an ordinary level.");
observed("stock", "FAIL", "ZDO-RTG-01", "Does not answer the request to list its routes.");
observed("stock", "FAIL", "att_write_cmd_not_answered", "Replies to writes sent in the form that must not receive a reply.");
observed("stock", "FAIL", "att_unsupported_request att_read_multiple att_read_multiple_variable", "Sends no reply, leaving the phone waiting until its request times out.");
observed("stock", "FAIL", "att_invalid_pdu_short att_invalid_pdu_long att_invalid_pdu_uuid_size", "Does not reject the malformed requests with the required error response.");
observed("stock", "FAIL", "att_error_rsp_format att_find_info_not_found", "Loses part of the value number in its error reply, so the reply points to the wrong place.");
observed("stock", "FAIL", "att_find_info_invalid_handles att_find_by_type_value_invalid_handles att_read_by_type_invalid_handles att_read_by_group_type_errors", "Accepts an invalid search range or replies with the wrong error information.");
observed("stock", "FAIL", "att_find_by_type_value_plain_attribute", "Repeats the same match and gives it an incorrect group boundary.");
observed("stock", "INCONCLUSIVE", "att_mtu_applied", "The available values and replies are too short to show whether the larger agreed message size is being used.");
observed("stock", "NOTE", "att_mtu_read_by_type_fill", "Keeps the reply small even though more matching values would fit in the agreed message size.");
observed("stock", "FAIL", "att_mtu_below_default", "Shrinks its replies below the standard minimum after the phone proposes an invalid limit.");
observed("outglow", "N/A", "att_mtu_server_request att_mtu_server_request_notifications", "Did not initiate message-size negotiation during the test, so this check does not apply. This result does not say whether it can answer a negotiation started by the phone.");
observed("stock", "PASS", "att_mtu_server_request", "Started message-size negotiation once and kept replies within the default limit until the phone answered.");
observed("stock", "FAIL", "att_mtu_server_request_notifications", "Sends an unsolicited value update before the phone has answered its message-size request.");
observed("stock", "NOTE", "att_read_by_type_uuid_forms", "Finds values using the short type identifier but fails to find them using its equivalent long form.");
observed("stock", "N/A", "att_read_by_type_not_readable", "Every available value is readable, so there is no unreadable value to exercise this case.");
observed("stock", "FAIL", "att_read_invalid_handles att_read_blob_invalid_handles att_write_invalid_handles att_prepare_write_errors", "Replies as if access were forbidden instead of reporting that the value number is invalid.");
observed("stock", "N/A", "att_read_long_value", "Has no value long enough to require reading in parts.");
observed("stock", "FAIL", "att_read_blob_invalid_offset", "Returns an empty value instead of rejecting a read beyond the end.");
observed("stock", "FAIL", "att_read_by_group_type_unsupported", "Returns misleading group information for values that are not service groups.");
observed("stock", "FAIL", "att_prepare_write", "Returns an error when asked to finish an empty queue after the staged changes have been cancelled.");
observed("stock", "N/A", "att_queued_write_execute", "The test has no suitable standard setting whose staged changes it can verify independently of the vendor app.");
observed("outglow", "N/A", "att_queued_write_execute att_prepare_write_errors", "Does not support staged writes and explicitly says so.");
observed("both", "NOTE", "gap_adv_no_gap_gatt_uuid", "Advertises a basic Bluetooth housekeeping service that the standard recommends leaving out; the vendor app expects it.");
observed("stock", "NOTE", "gap_param_update_pause", "Asks to change connection timing sooner than recommended.");
observed("stock", "FAIL", "gatt_cccd_present", "Sends value updates but does not provide the standard subscription setting.");
observed("stock", "N/A", "gatt_cccd_default gatt_cccd_write gatt_cccd_write_length gatt_cccd_bits_follow_properties", "Does not provide the standard subscription setting, so this setting cannot be tested.");
observed("outglow", "NOTE", "gatt_cccd_bits_follow_properties", "Accepts a subscription mode for confirmed updates even though that mode is not advertised as supported.");
observed("stock", "FAIL", "gatt_cccd_gates_notifications", "Sends updates without a standard subscription setting that the phone could enable.");
observed("outglow", "FAIL", "gatt_cccd_gates_notifications", "Sends updates even with the standard subscription switched off. It follows the vendor app's separate subscription command.");
observed("stock", "FAIL", "gatt_char_declaration_value", "Returns only part of a value's description when read directly, unlike the complete description returned during discovery.");
observed("both", "N/A", "gatt_service_gatt", "Has no service-change notification and does not pair, so no phone keeps a stored copy of its services between connections that a change could leave out of date.");
observed("stock", "FAIL", "gatt_gap_name_advertised", "Advertises itself as unpaired but returns the bulb's model name when the phone reads its device name.");
observed("both", "NOTE", "gatt_props_write", "Accepts a write request to the update channel even though its advertised permissions do not allow that form of write.");
observed("stock", "NOTE", "gatt_props_write_without_response", "Changes a value after a write that requires no reply, despite not advertising permission for that form of write.");
observed("stock", "NOTE", "gatt_user_description", "Accepts writes to text labels without advertising that they can be changed.");
observed("stock", "INCONCLUSIVE", "l2cap_fragmentation", "Produces no response long enough for the test to observe splitting it across radio packets.");
observed("stock", "FAIL", "l2cap_length_mismatch", "Processes a message containing more data than its declared length.");
observed("stock", "FAIL", "l2cap_sig_reject_unknown_code l2cap_sig_reject_disallowed_code l2cap_sig_credit_based_requests l2cap_sig_identifier l2cap_sig_param_update_req_to_peripheral l2cap_sig_mtu_exceeded", "Leaves the channel-management request unanswered.");
observed("stock", "FAIL", "ll_adv_scan_req_other_addr_type", "Answers a discovery request for a different address category when the address digits happen to match.");
observed("stock", "FAIL", "ll_adv_connect_ind_other_addr_type", "Starts a connection meant for a different address category when the address digits happen to match.");
observed("stock", "FAIL", "ll_conn_not_established_six_events", "Waits several seconds before giving up a connection that never starts, longer than the allowed opening deadline.");
observed("stock", "FAIL", "ll_ctrl_conn_update_at_instant", "Loses synchronisation with the phone when the timing change arrives exactly at its changeover moment.");
observed("stock", "FAIL", "ll_ctrl_conn_update_instant_passed", "Keeps the connection running instead of ending it after an overdue timing-change instruction.");
observed("stock", "FAIL", "ll_ctrl_version_once", "Sends its version information repeatedly within the same connection.");
observed("outglow", "N/A", "ll_ctrl_own_length_req", "Did not request longer radio packets during the test. This check does not assess its response to a request from the phone.");
observed("stock", "PASS", "ll_ctrl_own_length_req", "Requested longer radio packets, advertised support for them and proposed limits allowed by the standard.");
observed("stock", "FAIL", "ll_ctrl_procedure_timeout", "Keeps the connection open after its unanswered control request should have timed out.");
observed("outglow", "N/A", "ll_ctrl_procedure_timeout", "Starts no connection-control procedure of its own in this test, so the timeout cannot be exercised.");

export function behaviour(t: Test, r: Result, side: Side): string {
  const v = r[side];
  // Enforce withholding before consulting editorial copy.
  if (v === "WITHHELD") return "This result is not published. No conclusion about this firmware can be drawn here.";
  const observation = OBSERVATIONS[`${t.id}:${side}:${v}`];
  if (observation) return observation;
  if (v === "PASS") return CHECKS[t.id] ?? "Meets the expectation in this check.";
  if (v === "N/A" && t.id.startsWith("ZCL-COLOR-")) return "This bulb does not offer adjustable white tone, so this check does not apply.";
  if (v === "N/A" && t.id.startsWith("ZCL-DIAG-")) return "Does not offer the standard Zigbee diagnostics service.";
  return {
    FAIL: "Did not meet this check's expectation. A plain-English explanation of this new result is not available yet.",
    ERROR: "The test encountered an error; this is not a confirmed observation of the bulb's behaviour.",
    NOTE: "The result needs qualification because the rule is a recommendation or has more than one reading.",
    INCONCLUSIVE: "The test did not establish whether this behaviour meets the expectation.",
    "N/A": "This check does not apply to the features or behaviour exercised on this firmware.",
  }[v];
}
