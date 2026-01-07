# Workflow Integration Guide

This guide explains how to integrate the ADK documentation audit system with existing development workflows, including spec-driven development, CI/CD pipelines, and team processes.

## ADK-Rust Spec-Driven Development Integration

The documentation audit system is designed to work seamlessly with ADK-Rust's spec-driven development workflow.

### Workflow Phases Integration

#### 1. Requirements Phase
During requirements gathering, the audit system helps ensure:
- All requirements are properly documented
- API requirements reference valid interfaces
- Version requirements are consistent

```bash
# Validate requirements documents
adk-doc-audit validate .kiro/specs/*/requirements.md
```

#### 2. Design Phase  
During design creation, validate:
- Design documents reference actual APIs
- Code examples in design compile
- Cross-references between documents work

```bash
# Validate design documents
adk-doc-audit validate .kiro/specs/*/design.md

# Check that design examples compile
adk-doc-audit audit --docs .kiro/specs --validate-code-examples
```

#### 3. Implementation Phase
During task execution, ensure:
- Implementation matches documented APIs
- New features are properly documented
- Examples stay current with code changes

```bash
# Run incremental audit after implementing tasks
adk-doc-audit incremental --changed docs/api-reference.md docs/examples.md
```

### Spec Workflow Hooks

Integrate audit checks at key workflow points:

```rust
// Example: Hook into spec task completion
use adk_doc_audit::{AuditOrchestrator, AuditConfig};

async fn on_task_completed(task_id: &str, spec_name: &str) -> Result<()> {
    let config = AuditConfig::builder()
        .workspace_path(".")
        .docs_path("docs")
        .build();
    
    let orchestrator = AuditOrchestrator::new(config)?;
    
    // Audit documentation related to the completed task
    let spec_docs = vec![
        format!(".kiro/specs/{}/requirements.md", spec_name),
        format!(".kiro/specs/{}/design.md", spec_name),
        format!("docs/features/{}.md", spec_name),
    ];
    
    let report = orchestrator.run_incremental_audit(&spec_docs).await?;
    
    if report.summary.critical_issues > 0 {
        println!("⚠️  Documentation issues found after task completion:");
        for issue in &report.issues {
            println!("  - {}: {}", issue.file_path.display(), issue.message);
        }
    }
    
    Ok(())
}
```

## CI/CD Pipeline Integration

### GitHub Actions

#### Full Pipeline Example

```yaml
name: Documentation Quality
on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]

jobs:
  docs-audit:
    runs-on: ubuntu-latest
    steps:
      - name: Checkout code
        uses: actions/checkout@v3
        with:
          fetch-depth: 0  # Need full history for changed file detection
      
      - name: Setup Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          components: rustfmt, clippy
      
      - name: Cache dependencies
        uses: actions/cache@v3
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
      
      - name: Install adk-doc-audit
        run: cargo install --path adk-doc-audit
      
      - name: Full audit on main branch
        if: github.ref == 'refs/heads/main'
        run: |
          adk-doc-audit audit \
            --workspace . \
            --docs docs \
            --format json \
            --output audit-report.json \
            --fail-on-critical
      
      - name: Incremental audit on PR
        if: github.event_name == 'pull_request'
        run: |
          # Get changed markdown files
          CHANGED_FILES=$(git diff --name-only origin/main...HEAD -- '*.md' | tr '\n' ' ')
          
          if [ -n "$CHANGED_FILES" ]; then
            echo "Auditing changed files: $CHANGED_FILES"
            adk-doc-audit incremental \
              --workspace . \
              --docs docs \
              --changed $CHANGED_FILES \
              --format json \
              --output pr-audit-report.json
          else
            echo "No markdown files changed"
          fi
      
      - name: Comment PR with audit results
        if: github.event_name == 'pull_request' && failure()
        uses: actions/github-script@v6
        with:
          script: |
            const fs = require('fs');
            
            if (fs.existsSync('pr-audit-report.json')) {
              const report = JSON.parse(fs.readFileSync('pr-audit-report.json', 'utf8'));
              
              let comment = '## 📋 Documentation Audit Results\n\n';
              comment += `- **Total issues**: ${report.summary.total_issues}\n`;
              comment += `- **Critical issues**: ${report.summary.critical_issues}\n`;
              comment += `- **Warning issues**: ${report.summary.warning_issues}\n\n`;
              
              if (report.issues.length > 0) {
                comment += '### Issues Found\n\n';
                for (const issue of report.issues.slice(0, 10)) {  // Limit to 10 issues
                  const severity = issue.severity === 'Critical' ? '❌' : '⚠️';
                  comment += `${severity} **${issue.file_path}:${issue.line_number || '?'}**\n`;
                  comment += `${issue.message}\n`;
                  if (issue.suggestion) {
                    comment += `💡 *${issue.suggestion}*\n`;
                  }
                  comment += '\n';
                }
                
                if (report.issues.length > 10) {
                  comment += `... and ${report.issues.length - 10} more issues\n`;
                }
              }
              
              github.rest.issues.createComment({
                issue_number: context.issue.number,
                owner: context.repo.owner,
                repo: context.repo.repo,
                body: comment
              });
            }
      
      - name: Upload audit reports
        if: always()
        uses: actions/upload-artifact@v3
        with:
          name: audit-reports
          path: |
            audit-report.json
            pr-audit-report.json
          retention-days: 30
      
      - name: Fail on critical issues
        if: always()
        run: |
          if [ -f "audit-report.json" ]; then
            CRITICAL=$(jq '.summary.critical_issues' audit-report.json)
            if [ "$CRITICAL" -gt 0 ]; then
              echo "❌ Found $CRITICAL critical documentation issues"
              exit 1
            fi
          fi
          
          if [ -f "pr-audit-report.json" ]; then
            CRITICAL=$(jq '.summary.critical_issues' pr-audit-report.json)
            if [ "$CRITICAL" -gt 0 ]; then
              echo "❌ Found $CRITICAL critical documentation issues in PR"
              exit 1
            fi
          fi
```

#### Scheduled Audit

```yaml
name: Weekly Documentation Audit
on:
  schedule:
    - cron: '0 9 * * 1'  # Every Monday at 9 AM UTC

jobs:
  comprehensive-audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      
      - name: Install adk-doc-audit
        run: cargo install --path adk-doc-audit
      
      - name: Run comprehensive audit
        run: |
          adk-doc-audit audit \
            --workspace . \
            --docs docs \
            --format markdown \
            --output weekly-audit-report.md \
            --severity info  # Include all issues
      
      - name: Create issue if problems found
        if: failure()
        uses: actions/github-script@v6
        with:
          script: |
            const fs = require('fs');
            const report = fs.readFileSync('weekly-audit-report.md', 'utf8');
            
            github.rest.issues.create({
              owner: context.repo.owner,
              repo: context.repo.repo,
              title: `Weekly Documentation Audit - ${new Date().toISOString().split('T')[0]}`,
              body: report,
              labels: ['documentation', 'audit', 'maintenance']
            });
```

### GitLab CI

```yaml
# .gitlab-ci.yml
stages:
  - build
  - test
  - docs-audit
  - deploy

docs-audit:
  stage: docs-audit
  image: rust:latest
  before_script:
    - cargo install --path adk-doc-audit
  script:
    - |
      if [ "$CI_PIPELINE_SOURCE" = "merge_request_event" ]; then
        # Incremental audit for MRs
        CHANGED_FILES=$(git diff --name-only $CI_MERGE_REQUEST_TARGET_BRANCH_SHA...HEAD -- '*.md' | tr '\n' ' ')
        if [ -n "$CHANGED_FILES" ]; then
          adk-doc-audit incremental --changed $CHANGED_FILES --format json --output mr-audit.json
        fi
      else
        # Full audit for main branch
        adk-doc-audit audit --format json --output audit-report.json
      fi
  artifacts:
    reports:
      junit: audit-report.json
    paths:
      - audit-report.json
      - mr-audit.json
    expire_in: 1 week
  rules:
    - if: $CI_COMMIT_BRANCH == "main"
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
```

### Jenkins Pipeline

```groovy
pipeline {
    agent any
    
    stages {
        stage('Checkout') {
            steps {
                checkout scm
            }
        }
        
        stage('Setup') {
            steps {
                sh 'cargo install --path adk-doc-audit'
            }
        }
        
        stage('Documentation Audit') {
            parallel {
                stage('Full Audit') {
                    when {
                        branch 'main'
                    }
                    steps {
                        sh '''
                            adk-doc-audit audit \
                                --workspace . \
                                --docs docs \
                                --format json \
                                --output audit-report.json
                        '''
                    }
                }
                
                stage('Incremental Audit') {
                    when {
                        changeRequest()
                    }
                    steps {
                        script {
                            def changedFiles = sh(
                                script: "git diff --name-only origin/main...HEAD -- '*.md' | tr '\\n' ' '",
                                returnStdout: true
                            ).trim()
                            
                            if (changedFiles) {
                                sh """
                                    adk-doc-audit incremental \
                                        --changed ${changedFiles} \
                                        --format json \
                                        --output pr-audit.json
                                """
                            }
                        }
                    }
                }
            }
        }
    }
    
    post {
        always {
            archiveArtifacts artifacts: '*.json', allowEmptyArchive: true
            
            script {
                if (fileExists('audit-report.json')) {
                    def report = readJSON file: 'audit-report.json'
                    if (report.summary.critical_issues > 0) {
                        error("Found ${report.summary.critical_issues} critical documentation issues")
                    }
                }
            }
        }
    }
}
```

## Git Hooks Integration

### Pre-commit Hook

```bash
#!/bin/bash
# .git/hooks/pre-commit

echo "Running documentation audit on staged files..."

# Get staged markdown files
STAGED_FILES=$(git diff --cached --name-only --diff-filter=ACM | grep '\.md$')

if [ -z "$STAGED_FILES" ]; then
    echo "No markdown files staged, skipping audit"
    exit 0
fi

echo "Auditing files: $STAGED_FILES"

# Run incremental audit on staged files
adk-doc-audit incremental \
    --changed $STAGED_FILES \
    --severity warning \
    --format console

AUDIT_EXIT_CODE=$?

if [ $AUDIT_EXIT_CODE -ne 0 ]; then
    echo ""
    echo "❌ Documentation audit failed!"
    echo "Fix the issues above or use 'git commit --no-verify' to skip this check."
    exit 1
fi

echo "✅ Documentation audit passed"
exit 0
```

### Pre-push Hook

```bash
#!/bin/bash
# .git/hooks/pre-push

echo "Running documentation audit before push..."

# Get the remote and branch being pushed to
remote="$1"
url="$2"

# Read stdin to get the refs being pushed
while read local_ref local_sha remote_ref remote_sha; do
    if [ "$local_sha" = "0000000000000000000000000000000000000000" ]; then
        # Branch is being deleted, skip audit
        continue
    fi
    
    if [ "$remote_sha" = "0000000000000000000000000000000000000000" ]; then
        # New branch, audit all markdown files
        CHANGED_FILES=$(find docs -name "*.md" | tr '\n' ' ')
    else
        # Existing branch, audit changed files
        CHANGED_FILES=$(git diff --name-only "$remote_sha..$local_sha" -- '*.md' | tr '\n' ' ')
    fi
    
    if [ -n "$CHANGED_FILES" ]; then
        echo "Auditing changed documentation files..."
        
        adk-doc-audit incremental \
            --changed $CHANGED_FILES \
            --severity critical \
            --format console
        
        if [ $? -ne 0 ]; then
            echo "❌ Documentation audit failed for push to $remote_ref"
            exit 1
        fi
    fi
done

echo "✅ Documentation audit passed"
exit 0
```

## IDE Integration

### VS Code Extension

Create a VS Code task for documentation auditing:

```json
// .vscode/tasks.json
{
    "version": "2.0.0",
    "tasks": [
        {
            "label": "Audit Documentation",
            "type": "shell",
            "command": "adk-doc-audit",
            "args": ["audit", "--format", "console"],
            "group": "build",
            "presentation": {
                "echo": true,
                "reveal": "always",
                "focus": false,
                "panel": "shared"
            },
            "problemMatcher": {
                "owner": "adk-doc-audit",
                "fileLocation": "relative",
                "pattern": {
                    "regexp": "^(.*?):(\\d+)\\s+\\[(Critical|Warning|Info)\\]\\s+(.*)$",
                    "file": 1,
                    "line": 2,
                    "severity": 3,
                    "message": 4
                }
            }
        },
        {
            "label": "Audit Current File",
            "type": "shell",
            "command": "adk-doc-audit",
            "args": ["validate", "${file}"],
            "group": "build",
            "presentation": {
                "echo": true,
                "reveal": "always",
                "focus": false,
                "panel": "shared"
            }
        }
    ]
}
```

### Vim/Neovim Integration

```vim
" Add to your .vimrc or init.vim

" Function to run audit on current file
function! AuditCurrentFile()
    let l:file = expand('%:p')
    if l:file =~ '\.md$'
        execute '!adk-doc-audit validate ' . shellescape(l:file)
    else
        echo 'Not a markdown file'
    endif
endfunction

" Function to run full audit
function! AuditAllDocs()
    execute '!adk-doc-audit audit --format console'
endfunction

" Key mappings
nnoremap <leader>da :call AuditAllDocs()<CR>
nnoremap <leader>df :call AuditCurrentFile()<CR>

" Auto-audit on save for markdown files
autocmd BufWritePost *.md call AuditCurrentFile()
```

## Team Workflow Integration

### Code Review Process

#### 1. PR Template Integration

```markdown
<!-- .github/pull_request_template.md -->
## Documentation Checklist

- [ ] All new APIs are documented
- [ ] Code examples compile and work
- [ ] Version references are updated
- [ ] Internal links are valid
- [ ] Documentation audit passes

### Documentation Audit Results

<!-- The CI will automatically add audit results here -->
```

#### 2. Review Guidelines

Create team guidelines for documentation reviews:

```markdown
# Documentation Review Guidelines

## Before Submitting PR

1. Run local audit: `adk-doc-audit incremental --changed <files>`
2. Fix all critical issues
3. Address warning issues when possible
4. Ensure examples compile: `cargo test --doc`

## During Review

1. Check that audit CI passes
2. Verify new features are documented
3. Test code examples manually if complex
4. Ensure documentation matches implementation

## After Merge

1. Monitor weekly audit reports
2. Address any new issues promptly
3. Update documentation when APIs change
```

### Documentation Maintenance

#### 1. Regular Audit Schedule

```bash
#!/bin/bash
# scripts/weekly-docs-maintenance.sh

echo "Running weekly documentation maintenance..."

# Full audit with detailed report
adk-doc-audit audit \
    --format markdown \
    --output weekly-audit-$(date +%Y-%m-%d).md \
    --severity info

# Check for undocumented APIs
adk-doc-audit audit \
    --detect-missing-documentation \
    --format json \
    --output missing-docs-$(date +%Y-%m-%d).json

echo "Maintenance complete. Check generated reports."
```

#### 2. Issue Tracking Integration

```python
# scripts/create-docs-issues.py
import json
import requests
import sys

def create_github_issue(repo, token, title, body, labels):
    """Create a GitHub issue for documentation problems."""
    url = f"https://api.github.com/repos/{repo}/issues"
    headers = {
        "Authorization": f"token {token}",
        "Accept": "application/vnd.github.v3+json"
    }
    data = {
        "title": title,
        "body": body,
        "labels": labels
    }
    response = requests.post(url, headers=headers, json=data)
    return response.json()

def process_audit_report(report_file, repo, token):
    """Process audit report and create issues for critical problems."""
    with open(report_file) as f:
        report = json.load(f)
    
    critical_issues = [
        issue for issue in report['issues'] 
        if issue['severity'] == 'Critical'
    ]
    
    # Group issues by file
    issues_by_file = {}
    for issue in critical_issues:
        file_path = issue['file_path']
        if file_path not in issues_by_file:
            issues_by_file[file_path] = []
        issues_by_file[file_path].append(issue)
    
    # Create issues for each file with problems
    for file_path, file_issues in issues_by_file.items():
        title = f"Documentation issues in {file_path}"
        
        body = f"## Critical Documentation Issues\n\n"
        body += f"Found {len(file_issues)} critical issues in `{file_path}`:\n\n"
        
        for issue in file_issues:
            body += f"### Line {issue.get('line_number', '?')}\n"
            body += f"**{issue['message']}**\n\n"
            if issue.get('suggestion'):
                body += f"💡 Suggestion: {issue['suggestion']}\n\n"
        
        body += f"\n---\n*Generated by documentation audit on {report['timestamp']}*"
        
        labels = ["documentation", "bug", "critical"]
        
        result = create_github_issue(repo, token, title, body, labels)
        print(f"Created issue #{result['number']}: {title}")

if __name__ == "__main__":
    if len(sys.argv) != 4:
        print("Usage: python create-docs-issues.py <report.json> <repo> <token>")
        sys.exit(1)
    
    report_file, repo, token = sys.argv[1:4]
    process_audit_report(report_file, repo, token)
```

## Monitoring and Metrics

### Audit Metrics Dashboard

```python
# scripts/audit-metrics.py
import json
import matplotlib.pyplot as plt
import pandas as pd
from datetime import datetime, timedelta

def analyze_audit_trends(report_files):
    """Analyze audit report trends over time."""
    data = []
    
    for report_file in report_files:
        with open(report_file) as f:
            report = json.load(f)
        
        data.append({
            'date': report['timestamp'],
            'total_issues': report['summary']['total_issues'],
            'critical_issues': report['summary']['critical_issues'],
            'warning_issues': report['summary']['warning_issues'],
            'coverage': report['summary']['coverage_percentage'],
            'total_files': report['summary']['total_files']
        })
    
    df = pd.DataFrame(data)
    df['date'] = pd.to_datetime(df['date'])
    df = df.sort_values('date')
    
    # Plot trends
    fig, axes = plt.subplots(2, 2, figsize=(15, 10))
    
    # Issues over time
    axes[0, 0].plot(df['date'], df['total_issues'], label='Total Issues')
    axes[0, 0].plot(df['date'], df['critical_issues'], label='Critical Issues')
    axes[0, 0].set_title('Issues Over Time')
    axes[0, 0].legend()
    
    # Coverage over time
    axes[0, 1].plot(df['date'], df['coverage'])
    axes[0, 1].set_title('Documentation Coverage %')
    
    # Issue distribution
    latest = df.iloc[-1]
    issue_types = ['Critical', 'Warning', 'Info']
    issue_counts = [
        latest['critical_issues'],
        latest['warning_issues'],
        latest['total_issues'] - latest['critical_issues'] - latest['warning_issues']
    ]
    axes[1, 0].pie(issue_counts, labels=issue_types, autopct='%1.1f%%')
    axes[1, 0].set_title('Current Issue Distribution')
    
    # Files vs issues
    axes[1, 1].scatter(df['total_files'], df['total_issues'])
    axes[1, 1].set_xlabel('Total Files')
    axes[1, 1].set_ylabel('Total Issues')
    axes[1, 1].set_title('Files vs Issues')
    
    plt.tight_layout()
    plt.savefig('audit-trends.png', dpi=300, bbox_inches='tight')
    print("Saved audit trends chart to audit-trends.png")

if __name__ == "__main__":
    import glob
    report_files = glob.glob('audit-report-*.json')
    if report_files:
        analyze_audit_trends(report_files)
    else:
        print("No audit report files found")
```

This comprehensive workflow integration ensures that documentation quality is maintained throughout the development lifecycle, from individual commits to production releases.