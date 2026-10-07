//! Exact typed signatures for the reviewed CRS 4.25.1 HPP dependency boundary.
//! Keep unknown revisions and custom modifications enabled. No source is reparsed here.
use super::*;
use crate::seclang::Phase::{RequestBody, RequestHeaders};
use crate::seclang::{LocalActionValue, Severity};
use LocalActionValue::{Id, Phase, Severity as SeverityValue};
use Severity::Critical;

const MEMBER_921170_0: MemberSignature = MemberSignature {
    collection: CollectionKind::ArgsNames,
    modifier: TargetModifier::None,
    selector: SelectorSignature::None,
    operator: OperatorKind::Rx,
    negated: false,
    operand: OperatorSignature::Regex("."),
    actions: &[
        ("id", ActionSignature::Local(Id(921170))),
        ("phase", ActionSignature::Local(Phase(RequestBody))),
        ("pass", ActionSignature::NoArgument),
        ("nolog", ActionSignature::NoArgument),
        ("tag", ActionSignature::Literal("application-multi")),
        ("tag", ActionSignature::Literal("language-multi")),
        ("tag", ActionSignature::Literal("platform-multi")),
        ("tag", ActionSignature::Literal("attack-protocol")),
        ("tag", ActionSignature::Literal("OWASP_CRS")),
        ("tag", ActionSignature::Literal("OWASP_CRS/PROTOCOL-ATTACK")),
        ("tag", ActionSignature::Literal("capec/1000/152/137/15/460")),
        ("ver", ActionSignature::Literal("OWASP_CRS/4.25.1")),
        (
            "setvar",
            ActionSignature::SetVar(SetVarOperation::Add, "paramcounter_%{MATCHED_VAR_NAME}", "1"),
        ),
    ],
    transformations: &[],
};

pub(super) const UNIT_921170: &[MemberSignature] = &[MEMBER_921170_0];

const MEMBER_921180_0: MemberSignature = MemberSignature {
    collection: CollectionKind::Tx,
    modifier: TargetModifier::None,
    selector: SelectorSignature::Regex("paramcounter_.*"),
    operator: OperatorKind::Gt,
    negated: false,
    operand: OperatorSignature::Integer(1, "1"),
    actions: &[
        ("id", ActionSignature::Local(Id(921180))),
        ("phase", ActionSignature::Local(Phase(RequestBody))),
        ("block", ActionSignature::NoArgument),
        (
            "msg",
            ActionSignature::Template("HTTP Parameter Pollution (%{MATCHED_VAR_NAME})"),
        ),
        (
            "logdata",
            ActionSignature::Template("Matched Data: %{MATCHED_VAR} found within %{MATCHED_VAR_NAME}: %{MATCHED_VAR}"),
        ),
        ("tag", ActionSignature::Literal("application-multi")),
        ("tag", ActionSignature::Literal("language-multi")),
        ("tag", ActionSignature::Literal("platform-multi")),
        ("tag", ActionSignature::Literal("attack-protocol")),
        ("tag", ActionSignature::Literal("paranoia-level/3")),
        ("tag", ActionSignature::Literal("OWASP_CRS")),
        ("tag", ActionSignature::Literal("OWASP_CRS/PROTOCOL-ATTACK")),
        ("tag", ActionSignature::Literal("capec/1000/152/137/15/460")),
        ("ver", ActionSignature::Literal("OWASP_CRS/4.25.1")),
        ("severity", ActionSignature::Local(SeverityValue(Critical))),
        (
            "setvar",
            ActionSignature::SetVar(
                SetVarOperation::Add,
                "http_violation_score",
                "%{tx.critical_anomaly_score}",
            ),
        ),
        (
            "setvar",
            ActionSignature::SetVar(
                SetVarOperation::Add,
                "inbound_anomaly_score_pl3",
                "%{tx.critical_anomaly_score}",
            ),
        ),
    ],
    transformations: &[],
};

pub(super) const UNIT_921180: &[MemberSignature] = &[MEMBER_921180_0];

const MEMBER_922110_0: MemberSignature = MemberSignature {
    collection: CollectionKind::Tx,
    modifier: TargetModifier::None,
    selector: SelectorSignature::Regex("MULTIPART_HEADERS_CONTENT_TYPES_*"),
    operator: OperatorKind::Rx,
    negated: true,
    operand: OperatorSignature::Regex("^(?:(?:\\*|[^!\"\\(\\),/:-\\?\\[-\\]\\{\\}]+)/(?:\\*|[^!\"\\(\\),/:-\\?\\[-\\]\\{\\}]+)|\\*)(?:[\\s\\x0b]*;[\\s\\x0b]*(?:charset[\\s\\x0b]*=[\\s\\x0b]*\"?(?:iso-8859-15?|utf-8|windows-1252)\\b\"?|(?:[^\\s\\x0b-\"\\(\\),/:-\\?\\[-\\]c\\{\\}]|c(?:[^!\"\\(\\),/:-\\?\\[-\\]h\\{\\}]|h(?:[^!\"\\(\\),/:-\\?\\[-\\]a\\{\\}]|a(?:[^!\"\\(\\),/:-\\?\\[-\\]r\\{\\}]|r(?:[^!\"\\(\\),/:-\\?\\[-\\]s\\{\\}]|s(?:[^!\"\\(\\),/:-\\?\\[-\\]e\\{\\}]|e[^!\"\\(\\),/:-\\?\\[-\\]t\\{\\}]))))))[^!\"\\(\\),/:-\\?\\[-\\]\\{\\}]*[\\s\\x0b]*=[\\s\\x0b]*[^!\\(\\),/:-\\?\\[-\\]\\{\\}]+);?)*(?:[\\s\\x0b]*,[\\s\\x0b]*(?:(?:\\*|[^!\"\\(\\),/:-\\?\\[-\\]\\{\\}]+)/(?:\\*|[^!\"\\(\\),/:-\\?\\[-\\]\\{\\}]+)|\\*)(?:[\\s\\x0b]*;[\\s\\x0b]*(?:charset[\\s\\x0b]*=[\\s\\x0b]*\"?(?:iso-8859-15?|utf-8|windows-1252)\\b\"?|(?:[^\\s\\x0b-\"\\(\\),/:-\\?\\[-\\]c\\{\\}]|c(?:[^!\"\\(\\),/:-\\?\\[-\\]h\\{\\}]|h(?:[^!\"\\(\\),/:-\\?\\[-\\]a\\{\\}]|a(?:[^!\"\\(\\),/:-\\?\\[-\\]r\\{\\}]|r(?:[^!\"\\(\\),/:-\\?\\[-\\]s\\{\\}]|s(?:[^!\"\\(\\),/:-\\?\\[-\\]e\\{\\}]|e[^!\"\\(\\),/:-\\?\\[-\\]t\\{\\}]))))))[^!\"\\(\\),/:-\\?\\[-\\]\\{\\}]*[\\s\\x0b]*=[\\s\\x0b]*[^!\\(\\),/:-\\?\\[-\\]\\{\\}]+);?)*)*$"),
    actions: &[
        ("id", ActionSignature::Local(Id(922110))),
        ("phase", ActionSignature::Local(Phase(RequestBody))),
        ("block", ActionSignature::NoArgument),
        ("capture", ActionSignature::NoArgument),
        ("t", ActionSignature::Transformation(TransformationKind::None)),
        ("t", ActionSignature::Transformation(TransformationKind::Lowercase)),
        ("msg", ActionSignature::Template("Illegal MIME Multipart Header content-type: charset parameter")),
        ("logdata", ActionSignature::Template("Matched Data: %{MATCHED_VAR} found within Content-Type multipart form")),
        ("tag", ActionSignature::Literal("application-multi")),
        ("tag", ActionSignature::Literal("language-multi")),
        ("tag", ActionSignature::Literal("platform-multi")),
        ("tag", ActionSignature::Literal("attack-multipart-header")),
        ("tag", ActionSignature::Literal("attack-protocol")),
        ("tag", ActionSignature::Literal("paranoia-level/1")),
        ("tag", ActionSignature::Literal("OWASP_CRS")),
        ("tag", ActionSignature::Literal("OWASP_CRS/MULTIPART-ATTACK")),
        ("tag", ActionSignature::Literal("capec/272/220")),
        ("ver", ActionSignature::Literal("OWASP_CRS/4.25.1")),
        ("severity", ActionSignature::Local(SeverityValue(Critical))),
        ("setvar", ActionSignature::SetVar(SetVarOperation::Add, "inbound_anomaly_score_pl1", "%{tx.critical_anomaly_score}")),
    ],
    transformations: &[TransformationKind::Lowercase],
};

pub(super) const UNIT_922110: &[MemberSignature] = &[MEMBER_922110_0];

const MEMBER_931130_0: MemberSignature = MemberSignature {
    collection: CollectionKind::Args,
    modifier: TargetModifier::None,
    selector: SelectorSignature::None,
    operator: OperatorKind::Rx,
    negated: false,
    operand: OperatorSignature::Regex("(?i)(?:(?:url|jar):)?(?:a(?:cap|f[ps]|ttachment)|b(?:eshare|itcoin|lob)|c(?:a(?:llto|p)|id|vs|ompress.(?:zlib|bzip2))|d(?:a(?:v|ta)|ict|n(?:s|tp))|e(?:d2k|xpect)|f(?:(?:ee)?d|i(?:le|nger|sh)|tps?)|g(?:it|o(?:pher)?|lob)|h(?:323|ttps?)|i(?:ax|cap|(?:ma|p)ps?|rc[6s]?)|ja(?:bbe)?r|l(?:dap[is]?|ocal_file)|m(?:a(?:ilto|ven)|ms|umble)|n(?:e(?:tdoc|ws)|fs|ntps?)|ogg|p(?:aparazzi|h(?:ar|p)|op(?:2|3s?)|r(?:es|oxy)|syc)|r(?:mi|sync|tm(?:f?p)?|ar)|s(?:3|ftp|ips?|m(?:[bs]|tps?)|n(?:ews|mp)|sh(?:2(?:.(?:s(?:hell|(?:ft|c)p)|exec|tunnel))?)?|vn(?:\\+ssh)?)|t(?:e(?:amspeak|lnet)|ftp|urns?)|u(?:dp|nreal|t2004)|v(?:entrilo|iew-source|nc)|w(?:ebcal|ss?)|x(?:mpp|ri)|zip)://?(?:[^@]+@)?([^/]*)"),
    actions: &[
        ("id", ActionSignature::Local(Id(931130))),
        ("phase", ActionSignature::Local(Phase(RequestBody))),
        ("block", ActionSignature::NoArgument),
        ("capture", ActionSignature::NoArgument),
        ("t", ActionSignature::Transformation(TransformationKind::None)),
        ("msg", ActionSignature::Template("Possible Remote File Inclusion (RFI) Attack: Off-Domain Reference/Link")),
        ("logdata", ActionSignature::Template("Matched Data: %{TX.0} found within %{tx.original_target}: %{MATCHED_VAR}")),
        ("tag", ActionSignature::Literal("application-multi")),
        ("tag", ActionSignature::Literal("language-multi")),
        ("tag", ActionSignature::Literal("platform-multi")),
        ("tag", ActionSignature::Literal("attack-rfi")),
        ("tag", ActionSignature::Literal("paranoia-level/2")),
        ("tag", ActionSignature::Literal("OWASP_CRS")),
        ("tag", ActionSignature::Literal("OWASP_CRS/ATTACK-RFI")),
        ("tag", ActionSignature::Literal("capec/1000/152/175/253")),
        ("ver", ActionSignature::Literal("OWASP_CRS/4.25.1")),
        ("severity", ActionSignature::Local(SeverityValue(Critical))),
        ("setvar", ActionSignature::SetVar(SetVarOperation::Assign, "rfi_parameter_%{MATCHED_VAR_NAME}", ".%{tx.1}")),
        ("setvar", ActionSignature::SetVar(SetVarOperation::Assign, "original_target", "%{MATCHED_VAR_NAME}")),
        ("chain", ActionSignature::NoArgument),
    ],
    transformations: &[],
};

const MEMBER_931130_1: MemberSignature = MemberSignature {
    collection: CollectionKind::Tx,
    modifier: TargetModifier::None,
    selector: SelectorSignature::Regex("rfi_parameter_.*"),
    operator: OperatorKind::EndsWith,
    negated: true,
    operand: OperatorSignature::String(".%{request_headers.host}"),
    actions: &[
        (
            "setvar",
            ActionSignature::SetVar(SetVarOperation::Add, "rfi_score", "%{tx.critical_anomaly_score}"),
        ),
        (
            "setvar",
            ActionSignature::SetVar(
                SetVarOperation::Add,
                "inbound_anomaly_score_pl2",
                "%{tx.critical_anomaly_score}",
            ),
        ),
    ],
    transformations: &[],
};

pub(super) const UNIT_931130: &[MemberSignature] = &[MEMBER_931130_0, MEMBER_931130_1];

const MEMBER_920450_0: MemberSignature = MemberSignature {
    collection: CollectionKind::RequestHeadersNames,
    modifier: TargetModifier::None,
    selector: SelectorSignature::None,
    operator: OperatorKind::Rx,
    negated: false,
    operand: OperatorSignature::Regex("^.*$"),
    actions: &[
        ("id", ActionSignature::Local(Id(920450))),
        ("phase", ActionSignature::Local(Phase(RequestHeaders))),
        ("block", ActionSignature::NoArgument),
        ("capture", ActionSignature::NoArgument),
        ("t", ActionSignature::Transformation(TransformationKind::None)),
        ("t", ActionSignature::Transformation(TransformationKind::Lowercase)),
        (
            "msg",
            ActionSignature::Template("HTTP header is restricted by policy (%{MATCHED_VAR})"),
        ),
        (
            "logdata",
            ActionSignature::Template("Restricted header detected: %{MATCHED_VAR}"),
        ),
        ("tag", ActionSignature::Literal("application-multi")),
        ("tag", ActionSignature::Literal("language-multi")),
        ("tag", ActionSignature::Literal("platform-multi")),
        ("tag", ActionSignature::Literal("attack-protocol")),
        ("tag", ActionSignature::Literal("paranoia-level/1")),
        ("tag", ActionSignature::Literal("OWASP_CRS")),
        ("tag", ActionSignature::Literal("OWASP_CRS/PROTOCOL-ENFORCEMENT")),
        ("tag", ActionSignature::Literal("capec/1000/210/272")),
        ("ver", ActionSignature::Literal("OWASP_CRS/4.25.1")),
        ("severity", ActionSignature::Local(SeverityValue(Critical))),
        (
            "setvar",
            ActionSignature::SetVar(SetVarOperation::Assign, "header_name_920450_%{tx.0}", "/%{tx.0}/"),
        ),
        ("chain", ActionSignature::NoArgument),
    ],
    transformations: &[TransformationKind::Lowercase],
};

const MEMBER_920450_1: MemberSignature = MemberSignature {
    collection: CollectionKind::Tx,
    modifier: TargetModifier::None,
    selector: SelectorSignature::Regex("^header_name_920450_"),
    operator: OperatorKind::Within,
    negated: false,
    operand: OperatorSignature::String("%{tx.restricted_headers_basic}"),
    actions: &[(
        "setvar",
        ActionSignature::SetVar(
            SetVarOperation::Add,
            "inbound_anomaly_score_pl1",
            "%{tx.critical_anomaly_score}",
        ),
    )],
    transformations: &[],
};

pub(super) const UNIT_920450: &[MemberSignature] = &[MEMBER_920450_0, MEMBER_920450_1];

const MEMBER_920451_0: MemberSignature = MemberSignature {
    collection: CollectionKind::RequestHeadersNames,
    modifier: TargetModifier::None,
    selector: SelectorSignature::None,
    operator: OperatorKind::Rx,
    negated: false,
    operand: OperatorSignature::Regex("^.*$"),
    actions: &[
        ("id", ActionSignature::Local(Id(920451))),
        ("phase", ActionSignature::Local(Phase(RequestHeaders))),
        ("block", ActionSignature::NoArgument),
        ("capture", ActionSignature::NoArgument),
        ("t", ActionSignature::Transformation(TransformationKind::None)),
        ("t", ActionSignature::Transformation(TransformationKind::Lowercase)),
        (
            "msg",
            ActionSignature::Template("HTTP header is restricted by policy (%{MATCHED_VAR})"),
        ),
        (
            "logdata",
            ActionSignature::Template("Restricted header detected: %{MATCHED_VAR}"),
        ),
        ("tag", ActionSignature::Literal("application-multi")),
        ("tag", ActionSignature::Literal("language-multi")),
        ("tag", ActionSignature::Literal("platform-multi")),
        ("tag", ActionSignature::Literal("attack-protocol")),
        ("tag", ActionSignature::Literal("paranoia-level/2")),
        ("tag", ActionSignature::Literal("OWASP_CRS")),
        ("tag", ActionSignature::Literal("OWASP_CRS/PROTOCOL-ENFORCEMENT")),
        ("tag", ActionSignature::Literal("capec/1000/210/272")),
        ("ver", ActionSignature::Literal("OWASP_CRS/4.25.1")),
        ("severity", ActionSignature::Local(SeverityValue(Critical))),
        (
            "setvar",
            ActionSignature::SetVar(SetVarOperation::Assign, "header_name_920451_%{tx.0}", "/%{tx.0}/"),
        ),
        ("chain", ActionSignature::NoArgument),
    ],
    transformations: &[TransformationKind::Lowercase],
};

const MEMBER_920451_1: MemberSignature = MemberSignature {
    collection: CollectionKind::Tx,
    modifier: TargetModifier::None,
    selector: SelectorSignature::Regex("^header_name_920451_"),
    operator: OperatorKind::Within,
    negated: false,
    operand: OperatorSignature::String("%{tx.restricted_headers_extended}"),
    actions: &[(
        "setvar",
        ActionSignature::SetVar(
            SetVarOperation::Add,
            "inbound_anomaly_score_pl2",
            "%{tx.critical_anomaly_score}",
        ),
    )],
    transformations: &[],
};

pub(super) const UNIT_920451: &[MemberSignature] = &[MEMBER_920451_0, MEMBER_920451_1];

const MEMBER_922140_0: MemberSignature = MemberSignature {
    collection: CollectionKind::MultipartPartHeaders,
    modifier: TargetModifier::Count,
    selector: SelectorSignature::None,
    operator: OperatorKind::Gt,
    negated: false,
    operand: OperatorSignature::Integer(0, "0"),
    actions: &[
        ("id", ActionSignature::Local(Id(922140))),
        ("phase", ActionSignature::Local(Phase(RequestBody))),
        ("pass", ActionSignature::NoArgument),
        ("t", ActionSignature::Transformation(TransformationKind::None)),
        ("nolog", ActionSignature::NoArgument),
        ("tag", ActionSignature::Literal("attack-multipart-header")),
        ("tag", ActionSignature::Literal("OWASP_CRS")),
        ("tag", ActionSignature::Literal("OWASP_CRS/MULTIPART-ATTACK")),
        ("ver", ActionSignature::Literal("OWASP_CRS/4.25.1")),
        (
            "setvar",
            ActionSignature::SetVar(SetVarOperation::Assign, "multipart_headers_content_counter", "0"),
        ),
    ],
    transformations: &[],
};

pub(super) const UNIT_922140: &[MemberSignature] = &[MEMBER_922140_0];

const MEMBER_922150_0: MemberSignature = MemberSignature {
    collection: CollectionKind::MultipartPartHeaders,
    modifier: TargetModifier::None,
    selector: SelectorSignature::None,
    operator: OperatorKind::Rx,
    negated: false,
    operand: OperatorSignature::Regex("^content-type\\s*:\\s*(.*)$"),
    actions: &[
        ("id", ActionSignature::Local(Id(922150))),
        ("phase", ActionSignature::Local(Phase(RequestBody))),
        ("pass", ActionSignature::NoArgument),
        ("capture", ActionSignature::NoArgument),
        ("t", ActionSignature::Transformation(TransformationKind::None)),
        ("t", ActionSignature::Transformation(TransformationKind::Lowercase)),
        ("nolog", ActionSignature::NoArgument),
        ("tag", ActionSignature::Literal("attack-multipart-header")),
        ("tag", ActionSignature::Literal("OWASP_CRS")),
        ("tag", ActionSignature::Literal("OWASP_CRS/MULTIPART-ATTACK")),
        ("ver", ActionSignature::Literal("OWASP_CRS/4.25.1")),
        (
            "setvar",
            ActionSignature::SetVar(
                SetVarOperation::Assign,
                "multipart_headers_content_types_%{tx.multipart_headers_content_counter}",
                "%{tx.1}",
            ),
        ),
        (
            "setvar",
            ActionSignature::SetVar(SetVarOperation::Add, "multipart_headers_content_counter", "1"),
        ),
    ],
    transformations: &[TransformationKind::Lowercase],
};

pub(super) const UNIT_922150: &[MemberSignature] = &[MEMBER_922150_0];
