//! Execute one desktop action and return a separate post-action observation.
use async_trait::async_trait;
use serde_json::{json,Value};
use crate::tools::types::{Tool,ToolResult,ToolCategory,ToolRiskTier,ToolUseContext,ValidationResult,PermissionResult};
use super::{desktop_runtime, input_control_tools, system_ops::ScreenshotAnalyzeTool};

pub struct ComputerActionTool;

fn legacy_tool(action:&str) -> Option<Box<dyn Tool>> {
    match action {
        "click"=>Some(Box::new(input_control_tools::ClickMouseTool::new())),
        "key"=>Some(Box::new(input_control_tools::HotkeyTool::new())),
        "type"=>Some(Box::new(input_control_tools::TypeTextTool::new())),
        _=>None,
    }
}

#[async_trait]
impl Tool for ComputerActionTool {
    fn name(&self)->&str { "computer_action" }
    fn description(&self)->&str {
        "Perform one desktop action (click, key, type, move, drag, scroll), then observe the screen by default. Optionally focus the observation on a physical-pixel region and a specific visual question. Coordinates are physical virtual-desktop pixels, including negative monitor origins. Input success does not prove the goal was achieved. Read observation before choosing the next action; an observation failure must not cause automatic replay."
    }
    fn description_in(&self,lang:&str)->&str {
        match lang {
            "zh"=>"执行一次桌面操作（click/key/type/move/drag/scroll），默认随后截屏识别；可指定观察区域与具体问题。坐标使用虚拟桌面物理像素，副屏可为负数。操作成功不等于目标完成；根据观察结果决定下一步，观察失败时不能自动重复操作。",
            _=>self.description(),
        }
    }
    fn parameters_schema(&self)->Value {
        json!({"type":"object","properties":{
            "action":{"type":"string","enum":["click","key","type","move","drag","scroll"]},
            "x":{"type":"integer"},"y":{"type":"integer"},
            "to_x":{"type":"integer"},"to_y":{"type":"integer"},
            "button":{"type":"string","enum":["left","right","double"]},
            "keys":{"type":"string"},"text":{"type":"string","maxLength":4000},
            "steps":{"type":"integer","minimum":-30,"maximum":30,"description":"Vertical wheel notches; positive scrolls down"},
            "observe_after":{"type":"boolean","default":true},
            "observe_region":{"type":"object","description":"Optional post-action crop in physical virtual-desktop coordinates.","properties":{"x":{"type":"integer"},"y":{"type":"integer"},"width":{"type":"integer","minimum":2},"height":{"type":"integer","minimum":2}},"required":["x","y","width","height"],"additionalProperties":false},
            "observe_question":{"type":"string","minLength":1,"maxLength":500,"description":"Specific visual result to check after the action."}
        },"required":["action"]})
    }
    async fn validate_input(&self,input:&Value,ctx:&ToolUseContext)->ValidationResult {
        if input.get("observe_after").is_some_and(|v|!v.is_boolean()) {return ValidationResult::failure("observe_after must be a boolean",2);}
        let mut inspection=serde_json::Map::new();
        if let Some(region)=input.get("observe_region") {inspection.insert("region".into(),region.clone());}
        if let Some(question)=input.get("observe_question") {inspection.insert("question".into(),question.clone());}
        let inspection=Value::Object(inspection);
        let inspection_valid=ScreenshotAnalyzeTool::new().validate_input(&inspection,ctx).await;
        if !inspection_valid.result {return inspection_valid;}
        if let Some(region)=input.get("observe_region") {
            let x=region["x"].as_i64().unwrap();
            let y=region["y"].as_i64().unwrap();
            let width=region["width"].as_u64().unwrap() as i64;
            let height=region["height"].as_u64().unwrap() as i64;
            let Some(right)=x.checked_add(width-1) else {return ValidationResult::failure("Observation region is outside the desktop",2);};
            let Some(bottom)=y.checked_add(height-1) else {return ValidationResult::failure("Observation region is outside the desktop",2);};
            if desktop_runtime::validate_point(x,y).is_err() || desktop_runtime::validate_point(right,bottom).is_err() {
                return ValidationResult::failure("Observation region is outside the desktop",2);
            }
        }
        let action=input["action"].as_str().unwrap_or("");
        if let Some(tool)=legacy_tool(action) { return tool.validate_input(input,ctx).await; }
        if !matches!(action,"move"|"drag"|"scroll") {return ValidationResult::failure("Unknown desktop action",2);}
        let Some(x)=input["x"].as_i64() else {return ValidationResult::failure("x must be an integer",2);};
        let Some(y)=input["y"].as_i64() else {return ValidationResult::failure("y must be an integer",2);};
        if let Err(error)=desktop_runtime::validate_point(x,y) {return ValidationResult::failure(error,2);}
        if action=="drag" {
            let (Some(x),Some(y))=(input["to_x"].as_i64(),input["to_y"].as_i64()) else {return ValidationResult::failure("Drag requires to_x/to_y",2);};
            if let Err(error)=desktop_runtime::validate_point(x,y) {return ValidationResult::failure(error,2);}
        }
        if action=="scroll" && !input["steps"].as_i64().is_some_and(|n|(-30..=30).contains(&n)) {
            return ValidationResult::failure("steps must be an integer from -30 to 30",2);
        }
        ValidationResult::success(Some(input.clone()))
    }
    async fn check_permissions(&self,args:&Value,_:&ToolUseContext)->PermissionResult {
        if args["observe_after"].as_bool().unwrap_or(true) {
            PermissionResult::ask("允许这次电脑操作并截屏识别操作后的画面？")
        } else { PermissionResult::allow() }
    }
    async fn call(&self,args:Value,ctx:&ToolUseContext)->ToolResult {
        let valid=self.validate_input(&args,ctx).await;
        if !valid.result {return ToolResult::standard_error(&valid.message,Some("InvalidDesktopAction"),None);}
        let action=args["action"].as_str().unwrap_or("");
        let perform=async {
        let result=if let Some(tool)=legacy_tool(action) {tool.call(args.clone(),ctx).await} else {
            let x=args["x"].as_i64().unwrap();let y=args["y"].as_i64().unwrap();
            let prefix="Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class D{[DllImport(\"user32.dll\")]public static extern bool SetCursorPos(int x,int y);[DllImport(\"user32.dll\")]public static extern void mouse_event(uint f,int x,int y,int d,UIntPtr e);}';";
            let move_to=format!("if(-not [D]::SetCursorPos({x},{y})){{throw 'Cannot move cursor'}};");
            let operation=match action {
                "drag"=>format!("[D]::mouse_event(2,0,0,0,[UIntPtr]::Zero);try{{$sx={x};$sy={y};for($i=1;$i -le 12;$i++){{[D]::SetCursorPos([int]($sx+({}- $sx)*$i/12),[int]($sy+({}- $sy)*$i/12))|Out-Null;Start-Sleep -Milliseconds 16}}}}finally{{[D]::mouse_event(4,0,0,0,[UIntPtr]::Zero)}}",args["to_x"].as_i64().unwrap(),args["to_y"].as_i64().unwrap()),
                "scroll"=>format!("[D]::mouse_event(2048,0,0,{},[UIntPtr]::Zero);",-120*args["steps"].as_i64().unwrap()),
                _=>String::new(),
            };
            match desktop_runtime::run_input(&format!("{prefix}{move_to}{operation}")).await {
                Ok(_)=>ToolResult::standard_success("Desktop input sent",None),
                Err(error)=>ToolResult::standard_error(&error,Some("DesktopInputFailed"),None),
            }
        };
        if !result.success { return Err(result.error.unwrap_or_else(||"Desktop input failed".into())); }
        Ok(result.data)
        };
        let observe=async {
            let mut inspection=serde_json::Map::new();
            if let Some(region)=args.get("observe_region") {inspection.insert("region".into(),region.clone());}
            if let Some(question)=args.get("observe_question") {inspection.insert("question".into(),question.clone());}
            let result=ScreenshotAnalyzeTool::new().call(Value::Object(inspection),ctx).await;
            if result.success {Ok(result.data)} else {Err(result.error.unwrap_or_else(||"Observation failed".into()))}
        };
        match crate::desktop_contract::run_observed_operation(perform,observe,args["observe_after"].as_bool().unwrap_or(true)).await {
            Ok(receipt)=>{
                let mut data=serde_json::to_value(receipt).unwrap();
                data["action"]=json!(action);
                data["coordinate_space"]=json!("physical_virtual_desktop");
                data["screen_bounds"]=json!(desktop_runtime::screen_bounds().ok());
                data["next_step"]=json!("Evaluate observation before another input; observe again if vision failed.");
                ToolResult::success(data)
            },
            Err(error)=>ToolResult::standard_error(&error,Some("DesktopInputFailed"),None),
        }
    }
    fn category(&self)->ToolCategory {ToolCategory::System}
    fn risk(&self)->ToolRiskTier {ToolRiskTier::InputControl}
    fn is_read_only(&self)->bool {false}
    fn is_destructive(&self)->bool {true}
    fn should_defer(&self)->bool {true}
    fn search_hint(&self)->&str {"computer desktop click drag scroll mouse keyboard 电脑操作 拖拽 滚动"}
}
