// what the parser learned about the hooks, generators read it after parsing,
// so a later %init can still change the expression of an earlier class

pub const UNGROUPED: &str = "_ungrouped";

pub struct Subclass {
    pub superclass: usize,
    pub protocols: Vec<String>,
}

pub struct Class {
    pub name: String,
    pub expression: Option<String>,
    pub meta_expression: Option<String>,
    pub has_meta_hooks: bool,
    pub has_instance_hooks: bool,
    pub overridden: bool,
    pub required: bool,
    pub methods: Vec<usize>,
    pub properties: Vec<usize>,
    pub group: usize,
    pub subclass: Option<Subclass>,
}

impl Class {
    fn new(name: &str, group: usize) -> Class {
        Class {
            name: name.to_string(),
            expression: None,
            meta_expression: None,
            has_meta_hooks: false,
            has_instance_hooks: false,
            overridden: false,
            required: false,
            methods: Vec::new(),
            properties: Vec::new(),
            group,
            subclass: None,
        }
    }

    pub fn set_expression(&mut self, expr: &str) {
        self.expression = Some(expr.to_string());
        self.overridden = true;
    }

    pub fn set_meta_expression(&mut self, expr: &str) {
        self.meta_expression = Some(expr.to_string());
        self.overridden = true;
    }

    /// c type of an instance, `id` once the class comes from an expression
    pub fn type_name(&self) -> String {
        if self.expression.is_some() {
            "id".to_string()
        } else {
            format!("{}*", self.name)
        }
    }

    pub fn init_required(&self) -> bool {
        self.subclass.is_some() || self.required || !self.methods.is_empty() || !self.properties.is_empty()
    }
}

pub struct Method {
    pub class: usize,
    pub scope: u8,
    pub ret: String,
    pub selector_parts: Vec<String>,
    pub arg_names: Vec<String>,
    pub arg_types: Vec<String>,
    pub is_new: bool,
    pub type_encoding: String,
    pub variadic: bool,
}

impl Method {
    pub fn num_args(&self) -> usize {
        self.arg_types.len()
    }

    pub fn selector(&self) -> String {
        if self.num_args() == 0 {
            self.selector_parts.first().cloned().unwrap_or_default()
        } else {
            format!("{}:", self.selector_parts.join(":"))
        }
    }

    pub fn new_selector(&self) -> String {
        if self.num_args() == 0 {
            self.selector_parts.first().cloned().unwrap_or_default()
        } else {
            format!("{}$", self.selector_parts.join("$"))
        }
    }

    /// arc method family, which decides ownership of the returned object
    pub fn family(&self) -> &'static str {
        let sel = self.selector();
        let id_return = self.ret == "id" || self.ret == "instancetype";
        let tail_ok = |prefix: &str| -> bool {
            match sel.strip_prefix(prefix) {
                Some(rest) => match rest.bytes().next() {
                    None => true,
                    Some(c) => c.is_ascii_uppercase() || c == b',' || c == b':',
                },
                None => false,
            }
        };
        if self.scope == b'+' {
            if tail_ok("alloc") && id_return {
                return "alloc";
            }
            if sel == "new" && id_return {
                return "new";
            }
        } else {
            if tail_ok("init") && id_return {
                return "init";
            }
            if sel == "copy" || sel == "copyWithZone:" {
                return "copy";
            }
            if sel == "mutableCopy" || sel == "mutableCopyWithZone:" {
                return "mutableCopy";
            }
        }
        ""
    }
}

pub struct Function {
    pub name: String,
    pub expression: Option<String>,
    pub retval: String,
    pub args: Vec<String>,
    pub group: usize,
}

pub struct Property {
    pub class: usize,
    pub group: usize,
    pub name: String,
    pub type_name: String,
    pub readonly: bool,
    pub policy: String,
    pub getter: String,
    pub setter: String,
    pub retain: bool,
    pub copy: bool,
    pub nonatomic: bool,
}

pub struct Group {
    pub name: String,
    pub explicit: bool,
    pub initialized: bool,
    pub init_line: i64,
    pub classes: Vec<usize>,
    pub functions: Vec<usize>,
}

/// classes looked up by name through `%c` or `%class`, and subclasses that only need a declaration
#[derive(Default)]
pub struct StaticClasses {
    pub used_classes: Vec<String>,
    pub used_meta_classes: Vec<String>,
    pub declared_only: Vec<String>,
    pub names: Vec<(String, String)>,
}

impl StaticClasses {
    fn remember(&mut self, class: &str, real_name: &str) {
        match self.names.iter_mut().find(|(k, _)| k == class) {
            Some(entry) => entry.1 = real_name.to_string(),
            None => self.names.push((class.to_string(), real_name.to_string())),
        }
    }

    pub fn add_used_class(&mut self, class: &str, real_name: &str) {
        if !self.used_classes.iter().any(|c| c == class) {
            self.used_classes.push(class.to_string());
        }
        self.remember(class, real_name);
    }

    pub fn add_used_meta_class(&mut self, class: &str, real_name: &str) {
        if !self.used_meta_classes.iter().any(|c| c == class) {
            self.used_meta_classes.push(class.to_string());
        }
        self.remember(class, real_name);
    }

    pub fn add_declared_only(&mut self, class: &str) {
        if !self.declared_only.iter().any(|c| c == class) {
            self.declared_only.push(class.to_string());
        }
    }

    pub fn real_name(&self, class: &str) -> String {
        self.names
            .iter()
            .find(|(k, _)| k == class)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| class.to_string())
    }
}

pub struct Model {
    pub groups: Vec<Group>,
    pub classes: Vec<Class>,
    pub methods: Vec<Method>,
    pub functions: Vec<Function>,
    pub properties: Vec<Property>,
    pub static_classes: StaticClasses,
}

impl Model {
    pub fn new() -> Model {
        let mut m = Model {
            groups: Vec::new(),
            classes: Vec::new(),
            methods: Vec::new(),
            functions: Vec::new(),
            properties: Vec::new(),
            static_classes: StaticClasses::default(),
        };
        m.groups.push(Group {
            name: UNGROUPED.to_string(),
            explicit: false,
            initialized: false,
            init_line: -1,
            classes: Vec::new(),
            functions: Vec::new(),
        });
        m
    }

    pub fn get_group(&self, name: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.name == name)
    }

    pub fn add_group(&mut self, name: &str) -> usize {
        self.groups.push(Group {
            name: name.to_string(),
            explicit: true,
            initialized: false,
            init_line: -1,
            classes: Vec::new(),
            functions: Vec::new(),
        });
        self.groups.len() - 1
    }

    pub fn class_named(&self, group: usize, name: &str) -> Option<usize> {
        self.groups[group].classes.iter().copied().find(|&c| self.classes[c].name == name)
    }

    pub fn add_class_named(&mut self, group: usize, name: &str) -> usize {
        if let Some(c) = self.class_named(group, name) {
            return c;
        }
        self.classes.push(Class::new(name, group));
        let id = self.classes.len() - 1;
        self.groups[group].classes.push(id);
        id
    }

    pub fn add_subclass(&mut self, group: usize, name: &str, superclass: usize) -> usize {
        let mut c = Class::new(name, group);
        c.overridden = true;
        c.subclass = Some(Subclass { superclass, protocols: Vec::new() });
        self.classes.push(c);
        let id = self.classes.len() - 1;
        self.groups[group].classes.push(id);
        id
    }

    pub fn function_named(&self, group: usize, name: &str) -> Option<usize> {
        self.groups[group].functions.iter().copied().find(|&f| self.functions[f].name == name)
    }

    pub fn group_init_required(&self, group: usize) -> bool {
        let g = &self.groups[group];
        g.classes.iter().any(|&c| self.classes[c].init_required()) || !g.functions.is_empty()
    }

    pub fn group_name_of_class(&self, class: usize) -> &str {
        &self.groups[self.classes[class].group].name
    }
}
